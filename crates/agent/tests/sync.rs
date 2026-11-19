mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use sing_portal_agent::state::LocalState;
use sing_portal_agent::{Agent, AgentConfig, Host, HostError, HostResult};
use sing_portal_core::protocol::{AgentMessage, ServerMessage};
use sing_portal_core::render::render;
use tempfile::TempDir;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{WebSocketStream, accept_async};

const TEMPLATE: &str = r#"{
  "log": { "level": "warn" },
  "route": { "final": "s_out" }
}"#;

/// The same side effects the desktop binary performs: a config file sing-box
/// reads and a command that reloads it.
struct TestHost {
    config_path: PathBuf,
    state_path: PathBuf,
    restart: Option<String>,
}

impl TestHost {
    fn new(config_path: PathBuf, state_path: PathBuf, restart: Option<String>) -> Self {
        Self {
            config_path,
            state_path,
            restart,
        }
    }

    fn written(&self) -> Value {
        serde_json::from_slice(&std::fs::read(&self.config_path).unwrap()).unwrap()
    }
}

impl Host for TestHost {
    fn write_config(&self, config: &[u8]) -> HostResult<()> {
        write_atomic(&self.config_path, config)
    }

    fn has_config(&self) -> bool {
        self.config_path.exists()
    }

    fn restart(&self) -> HostResult<()> {
        let Some(command) = self.restart.as_deref() else {
            return Ok(());
        };
        let output = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(command)
            .output()?;
        if !output.status.success() {
            return Err(HostError::new(format!(
                "restart command failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(())
    }

    fn load_state(&self) -> Option<LocalState> {
        let bytes = std::fs::read(&self.state_path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn save_state(&self, state: &LocalState) -> HostResult<()> {
        write_atomic(&self.state_path, &serde_json::to_vec_pretty(state)?)
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> HostResult<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

fn outbounds() -> Value {
    json!([
        { "type": "socks", "tag": "node-a", "server": "127.0.0.1", "server_port": 1 },
        { "type": "socks", "tag": "node-b", "server": "127.0.0.1", "server_port": 2 }
    ])
}

async fn next_message(socket: &mut WebSocketStream<TcpStream>) -> AgentMessage {
    let frame = tokio::time::timeout(Duration::from_secs(10), socket.next())
        .await
        .expect("agent reply timed out")
        .expect("socket closed")
        .expect("socket error");
    match frame {
        Message::Text(text) => serde_json::from_str(&text).expect("valid agent message"),
        other => panic!("unexpected frame: {other:?}"),
    }
}

async fn send<M: serde::Serialize>(socket: &mut WebSocketStream<TcpStream>, message: &M) {
    socket
        .send(Message::Text(
            serde_json::to_string(message).unwrap().into(),
        ))
        .await
        .unwrap();
}

#[tokio::test]
async fn syncs_state_over_websocket() {
    if common::sing_box().is_none() {
        eprintln!("SING_BOX_BIN is not set, skipping");
        return;
    }

    let dir = TempDir::new().unwrap();
    let endpoint = common::local_endpoint().await;
    let ports = common::free_ports(4);
    let api_port = ports[0];
    let test_inbound = ports[1];
    let egress_inbound = ports[2];
    let health_inbound = ports[3];
    let test_url = format!("http://127.0.0.1:{endpoint}/");

    let template: Value = serde_json::from_str(TEMPLATE).unwrap();
    let template = {
        let mut template = template.clone();
        template["services"] = json!([
            { "type": "api", "tag": "s_api", "listen": "127.0.0.1", "listen_port": api_port }
        ]);
        template
    };

    let run = common::start(dir.path(), api_port, &{
        let mut initial = render(&template, &[], &test_url).unwrap();
        initial["log"] = json!({ "level": "warn" });
        initial
    })
    .await;
    let (mut box_child, config_dir, state_dir) = run;
    let pid = box_child.id().unwrap();
    let config_path = config_dir.join("config.json");

    let host = Arc::new(TestHost::new(
        config_path.clone(),
        state_dir.join("state.json"),
        Some(format!("kill -HUP {pid}")),
    ));

    let config = AgentConfig {
        portal_url: String::new(),
        node_name: "home".into(),
        token: None,
        template: serde_json::to_string(&template).unwrap(),
        api_url: format!("http://127.0.0.1:{api_port}"),
        latency_url: test_url.clone(),
        interval: Duration::from_secs(15),
        once: true,
        best_effort: false,
        auto_measure: true,
        health_timeout: Duration::from_secs(5),
        egress_health_interval: Duration::ZERO,
        egress_health_failures: 2,
        tester: sing_portal_agent::tester::TesterConfig {
            inbound: std::net::SocketAddr::from(([127, 0, 0, 1], test_inbound)),
            egress_inbound: std::net::SocketAddr::from(([127, 0, 0, 1], egress_inbound)),
            health_inbound: std::net::SocketAddr::from(([127, 0, 0, 1], health_inbound)),
            ..sing_portal_agent::tester::TesterConfig::default()
        },
    };

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(stream).await.unwrap();

        let hello = next_message(&mut socket).await;
        assert!(matches!(
            hello,
            AgentMessage::Hello { ref name, .. } if name == "home"
        ));

        send(
            &mut socket,
            &ServerMessage::Sync {
                id: "sync-1".into(),
                revision: "sha256:new".into(),
                selection_revision: "sha256:selection".into(),
                selection: "node-a".into(),
                outbounds: outbounds().as_array().unwrap().clone(),
            },
        )
        .await;

        next_message(&mut socket).await
    });

    let mut config = config;
    config.portal_url = format!("http://{address}");
    let agent = Agent::new(config.clone(), host.clone()).unwrap();
    agent.run().await.unwrap();

    let reply = server.await.unwrap();
    let AgentMessage::SyncResult {
        revision,
        selection,
        health,
        applied,
        error,
        ..
    } = reply
    else {
        panic!("expected sync result, got {reply:?}");
    };
    if !error.is_empty() {
        eprintln!(
            "sing-box log:\n{}",
            std::fs::read_to_string(dir.path().join("sing-box.log")).unwrap_or_default()
        );
    }
    assert_eq!(
        revision, "sha256:new",
        "health={health} applied={applied} error={error}"
    );
    assert_eq!(selection, "node-a");
    assert_eq!(health, "ok");
    assert!(applied);
    assert!(error.is_empty());

    let written = host.written();
    assert_eq!(written["outbounds"][0]["tag"], "s_out");

    let api = sing_portal_agent::api::BoxApi::new(&config.api_url, Duration::from_secs(5)).unwrap();
    assert_eq!(api.current_selection().await.unwrap(), "node-a");

    let _ = tokio::time::timeout(Duration::from_secs(2), box_child.kill()).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), box_child.wait()).await;
}

#[tokio::test]
async fn once_is_best_effort_when_portal_is_down() {
    let closed = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap()
    };

    let (mut config, host) = minimal_config();
    config.portal_url = format!("http://{closed}");
    config.best_effort = true;
    let agent = Agent::new(config, host.clone()).unwrap();
    assert!(agent.run().await.is_ok());

    let (mut config, host) = minimal_config();
    config.portal_url = format!("http://{closed}");
    config.best_effort = false;
    let agent = Agent::new(config, host).unwrap();
    assert!(agent.run().await.is_err());
}

fn minimal_config() -> (AgentConfig, Arc<TestHost>) {
    let dir = TempDir::new().unwrap();
    let host = Arc::new(TestHost::new(
        dir.path().join("config.json"),
        dir.path().join("state.json"),
        None,
    ));
    let config = AgentConfig {
        portal_url: String::new(),
        node_name: "home".into(),
        token: None,
        template: TEMPLATE.into(),
        api_url: "http://127.0.0.1:1".into(),
        latency_url: "http://127.0.0.1:1/".into(),
        interval: Duration::from_secs(15),
        once: true,
        best_effort: false,
        auto_measure: true,
        health_timeout: Duration::from_millis(500),
        egress_health_interval: Duration::ZERO,
        egress_health_failures: 2,
        tester: sing_portal_agent::tester::TesterConfig {
            inbound: std::net::SocketAddr::from(([127, 0, 0, 1], common::free_port())),
            egress_inbound: std::net::SocketAddr::from(([127, 0, 0, 1], common::free_port())),
            health_inbound: std::net::SocketAddr::from(([127, 0, 0, 1], common::free_port())),
            ..sing_portal_agent::tester::TesterConfig::default()
        },
    };
    (config, host)
}
