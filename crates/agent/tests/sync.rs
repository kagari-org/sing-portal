mod common;

use std::path::PathBuf;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use sing_portal_agent::{Agent, AgentConfig};
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
    let template_path = dir.path().join("template.json");
    std::fs::write(&template_path, serde_json::to_vec(&template).unwrap()).unwrap();

    let run = common::start(dir.path(), api_port, &{
        let mut initial = render(&template, &[], &test_url).unwrap();
        initial["log"] = json!({ "level": "warn" });
        initial
    })
    .await;
    let (mut box_child, config_dir, state_dir) = run;
    let pid = box_child.id().unwrap();
    let config_path = config_dir.join("config.json");

    let config = AgentConfig {
        portal_url: String::new(),
        node_name: "home".into(),
        template_path,
        config_path,
        state_dir: state_dir.clone(),
        api_url: format!("http://127.0.0.1:{api_port}"),
        latency_url: test_url.clone(),
        restart_cmd: Some(format!("kill -HUP {pid}")),
        interval: Duration::from_secs(15),
        once: true,
        best_effort: false,
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
    let agent = Agent::new(config.clone()).unwrap();
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

    let written: Value =
        serde_json::from_slice(&std::fs::read(&config.config_path).unwrap()).unwrap();
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

    let mut config = minimal_config().await;
    config.portal_url = format!("http://{closed}");
    config.best_effort = true;
    let agent = Agent::new(config).unwrap();
    assert!(agent.run().await.is_ok());

    let mut config = minimal_config().await;
    config.portal_url = format!("http://{closed}");
    config.best_effort = false;
    let agent = Agent::new(config).unwrap();
    assert!(agent.run().await.is_err());
}

async fn minimal_config() -> AgentConfig {
    let dir = TempDir::new().unwrap();
    let base = dir.keep();
    let path: PathBuf = base.join("template.json");
    std::fs::write(&path, TEMPLATE).unwrap();
    let config = AgentConfig {
        portal_url: String::new(),
        node_name: "home".into(),
        template_path: path,
        config_path: base.join("config.json"),
        state_dir: base.join("state"),
        api_url: "http://127.0.0.1:1".into(),
        latency_url: "http://127.0.0.1:1/".into(),
        restart_cmd: None,
        interval: Duration::from_secs(15),
        once: true,
        best_effort: false,
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
    config
}
