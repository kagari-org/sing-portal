use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use sing_portal_agent::{Agent, AgentConfig};
use sing_portal_core::protocol::{AgentMessage, ServerMessage};
use tempfile::TempDir;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{WebSocketStream, accept_async};

const TEMPLATE: &str = r#"{
  "log": { "level": "warn" },
  "inbounds": [
    { "type": "mixed", "tag": "mixed-in", "listen": "127.0.0.1", "listen_port": 7897 }
  ],
  "route": { "final": "s_out" }
}"#;

fn outbounds() -> Value {
    json!([
        { "type": "vless", "tag": "provider · US 01", "server": "1.2.3.4", "server_port": 443, "uuid": "u" },
        { "type": "vless", "tag": "provider · JP 02", "server": "5.6.7.8", "server_port": 443, "uuid": "u" }
    ])
}

struct Fixture {
    dir: TempDir,
    config: AgentConfig,
}

async fn fixture(portal: String) -> Fixture {
    let dir = TempDir::new().unwrap();
    let template_path = dir.path().join("template.json");
    std::fs::write(&template_path, TEMPLATE).unwrap();

    let config = AgentConfig {
        portal_url: portal,
        node_name: "home".into(),
        template_path,
        config_path: dir.path().join("run/config.json"),
        state_dir: dir.path().join("state"),
        sing_box_bin: fake_sing_box(&dir),
        api_url: "http://127.0.0.1:1".into(),
        restart_cmd: None,
        interval: Duration::from_secs(15),
        once: true,
        best_effort: false,
        health_timeout: Duration::from_millis(600),
        tester: sing_portal_agent::tester::TesterConfig::default(),
    };

    Fixture { dir, config }
}

fn fake_sing_box(dir: &TempDir) -> String {
    let path = dir.path().join("fake-sing-box");
    std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(&path, permissions).unwrap();
    path.to_string_lossy().into_owned()
}

async fn next_message(socket: &mut WebSocketStream<TcpStream>) -> AgentMessage {
    let frame = tokio::time::timeout(Duration::from_secs(5), socket.next())
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
        .send(Message::Text(serde_json::to_string(message).unwrap().into()))
        .await
        .unwrap();
}

#[tokio::test]
async fn syncs_state_over_websocket() {
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
                selection: "provider · US 01".into(),
                outbounds: outbounds().as_array().unwrap().clone(),
            },
        )
        .await;

        let reply = next_message(&mut socket).await;
        reply
    });

    let fixture = fixture(format!("http://{address}")).await;
    let agent = Agent::new(fixture.config.clone()).unwrap();
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
    assert_eq!(revision, "sha256:new");
    assert_eq!(selection, "provider · US 01");
    assert_eq!(health, "ok");
    assert!(applied);
    assert!(error.is_empty());

    let config: Value =
        serde_json::from_slice(&std::fs::read(&fixture.config.config_path).unwrap()).unwrap();
    assert_eq!(config["outbounds"][0]["tag"], "s_out");
    assert!(
        std::fs::metadata(fixture.config.outbounds_path()).is_ok(),
        "outbounds cache written"
    );
    let state: Value =
        serde_json::from_slice(&std::fs::read(fixture.config.local_state_path()).unwrap()).unwrap();
    assert_eq!(state["appliedRevision"], "sha256:new");
}

#[tokio::test]
async fn once_is_best_effort_when_portal_is_down() {
    let closed = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap()
    };

    let mut config = fixture(format!("http://{closed}")).await.config;
    config.best_effort = true;
    let agent = Agent::new(config).unwrap();
    assert!(agent.run().await.is_ok());

    let mut config = fixture(format!("http://{closed}")).await.config;
    config.best_effort = false;
    let agent = Agent::new(config).unwrap();
    assert!(agent.run().await.is_err());
}

