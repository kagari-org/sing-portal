use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};
use sing_portal_agent::{Agent, AgentConfig};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TEMPLATE: &str = r#"{
  "log": { "level": "warn" },
  "inbounds": [
    { "type": "mixed", "tag": "mixed-in", "listen": "127.0.0.1", "listen_port": 7897 }
  ],
  "route": { "final": "s_out" }
}"#;

fn state_body(selection: &str) -> Value {
    json!({
        "revision": "sha256:new",
        "selectionRevision": "sha256:sel",
        "selection": selection,
        "outbounds": [
            { "type": "vless", "tag": "jichang-a · US 01", "server": "1.2.3.4", "server_port": 443, "uuid": "u" },
            { "type": "vless", "tag": "jichang-a · JP 02", "server": "5.6.7.8", "server_port": 443, "uuid": "u" }
        ]
    })
}

struct Fixture {
    dir: TempDir,
    config: AgentConfig,
    portal: MockServer,
    clash: MockServer,
}

async fn fixture(selection: &str) -> Fixture {
    let dir = TempDir::new().unwrap();
    let portal = MockServer::start().await;
    let clash = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/nodes/home/state"))
        .respond_with(ResponseTemplate::new(200).set_body_json(state_body(selection)))
        .mount(&portal)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/nodes/home/status"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&portal)
        .await;

    Mock::given(method("GET"))
        .and(path("/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"version": "1.14.3"})))
        .mount(&clash)
        .await;

    Mock::given(method("GET"))
        .and(path("/proxies/s_out"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"now": "s_auto"})))
        .mount(&clash)
        .await;

    Mock::given(method("PUT"))
        .and(path("/proxies/s_out"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&clash)
        .await;

    let template_path = dir.path().join("template.json");
    std::fs::write(&template_path, TEMPLATE).unwrap();
    let sing_box = fake_sing_box(&dir, true);

    let config = AgentConfig {
        portal_url: portal.uri(),
        node_name: "home".into(),
        template_path,
        config_path: dir.path().join("run/config.json"),
        state_dir: dir.path().join("state"),
        sing_box_bin: sing_box,
        clash_url: clash.uri(),
        restart_cmd: Some("true".into()),
        interval: Duration::from_secs(15),
        once: true,
        best_effort: false,
        health_timeout: Duration::from_millis(600),
    };

    Fixture {
        dir,
        config,
        portal,
        clash,
    }
}

fn fake_sing_box(dir: &TempDir, success: bool) -> String {
    let path = dir.path().join("fake-sing-box");
    let script = if success {
        "#!/bin/sh\nexit 0\n"
    } else {
        "#!/bin/sh\necho 'FATAL decode config: unknown field' >&2\nexit 1\n"
    };
    std::fs::write(&path, script).unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(&path, permissions).unwrap();
    path.to_string_lossy().into_owned()
}

#[tokio::test]
async fn applies_state_and_reports() {
    let fixture = fixture("jichang-a · US 01").await;
    std::fs::create_dir_all(&fixture.config.state_dir).unwrap();

    let agent = Agent::new(fixture.config.clone()).unwrap();
    let outcome = agent.run_once().await.unwrap();

    assert_eq!(outcome.health, "ok");
    assert!(outcome.applied);
    assert_eq!(outcome.revision, "sha256:new");
    assert_eq!(outcome.selection, "jichang-a · US 01");

    let config: Value = serde_json::from_slice(
        &std::fs::read(&fixture.config.config_path).unwrap(),
    )
    .unwrap();
    let outbounds = config["outbounds"].as_array().unwrap();
    assert_eq!(outbounds[0]["tag"], "s_out");
    assert_eq!(outbounds[1]["tag"], "s_auto");
    assert_eq!(outbounds[2]["tag"], "s_direct");
    assert_eq!(outbounds.len(), 5);

    let state: Value =
        serde_json::from_slice(&std::fs::read(fixture.config.local_state_path()).unwrap()).unwrap();
    assert_eq!(state["appliedRevision"], "sha256:new");
    assert_eq!(state["appliedSelection"], "jichang-a · US 01");

    let reports = fixture
        .portal
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.url.path().ends_with("/status"))
        .count();
    assert_eq!(reports, 1);
}

#[tokio::test]
async fn failed_check_keeps_previous_outbounds() {
    let fixture = fixture("s_auto").await;
    std::fs::create_dir_all(&fixture.config.state_dir).unwrap();

    let previous = json!([{ "type": "direct", "tag": "old" }]);
    std::fs::write(
        fixture.config.outbounds_path(),
        serde_json::to_vec(&previous).unwrap(),
    )
    .unwrap();

    let mut config = fixture.config.clone();
    config.sing_box_bin = fake_sing_box(&fixture.dir, false);
    let agent = Agent::new(config).unwrap();
    let outcome = agent.run_once().await.unwrap();

    assert_eq!(outcome.health, "rollback");
    assert!(!outcome.applied);
    assert!(
        outcome.error.contains("unknown field"),
        "unexpected error: {}",
        outcome.error
    );

    let cached: Value =
        serde_json::from_slice(&std::fs::read(fixture.config.outbounds_path()).unwrap()).unwrap();
    assert_eq!(cached, previous);
}

#[tokio::test]
async fn unhealthy_sing_box_rolls_back() {
    let fixture = fixture("s_auto").await;
    std::fs::create_dir_all(&fixture.config.state_dir).unwrap();

    let previous = json!([{ "type": "direct", "tag": "old" }]);
    std::fs::write(
        fixture.config.outbounds_path(),
        serde_json::to_vec(&previous).unwrap(),
    )
    .unwrap();

    fixture.clash.reset().await;
    Mock::given(method("GET"))
        .and(path("/version"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&fixture.clash)
        .await;

    let agent = Agent::new(fixture.config.clone()).unwrap();
    let outcome = agent.run_once().await.unwrap();

    assert_eq!(outcome.health, "rollback");
    let cached: Value =
        serde_json::from_slice(&std::fs::read(fixture.config.outbounds_path()).unwrap()).unwrap();
    assert_eq!(cached, previous);
}

#[tokio::test]
async fn unknown_selection_falls_back_to_auto() {
    let fixture = fixture("removed · node").await;
    std::fs::create_dir_all(&fixture.config.state_dir).unwrap();

    let agent = Agent::new(fixture.config.clone()).unwrap();
    let outcome = agent.run_once().await.unwrap();

    assert_eq!(outcome.selection, "s_auto");
    let puts = fixture
        .clash
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.method.as_str() == "PUT")
        .count();
    assert_eq!(puts, 0, "selection already s_auto, no update needed");
}

#[tokio::test]
async fn not_modified_keeps_cache() {
    let fixture = fixture("s_auto").await;
    std::fs::create_dir_all(&fixture.config.state_dir).unwrap();

    let cached = json!([{ "type": "vless", "tag": "cached" }]);
    std::fs::write(
        fixture.config.outbounds_path(),
        serde_json::to_vec(&cached).unwrap(),
    )
    .unwrap();
    std::fs::write(
        fixture.config.local_state_path(),
        serde_json::to_vec(&json!({
            "appliedRevision": "sha256:new",
            "appliedSelection": "s_auto",
            "lastError": "",
            "updatedAt": 0
        }))
        .unwrap(),
    )
    .unwrap();

    fixture.portal.reset().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/nodes/home/state"))
        .respond_with(ResponseTemplate::new(304))
        .mount(&fixture.portal)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/nodes/home/status"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&fixture.portal)
        .await;

    let agent = Agent::new(fixture.config.clone()).unwrap();
    let outcome = agent.run_once().await.unwrap();

    assert_eq!(outcome.health, "ok");
    assert!(!outcome.applied);
    let cached_after: Value =
        serde_json::from_slice(&std::fs::read(fixture.config.outbounds_path()).unwrap()).unwrap();
    assert_eq!(cached_after, cached);
}

#[tokio::test]
async fn portal_outage_is_degraded_but_not_fatal() {
    let fixture = fixture("s_auto").await;
    std::fs::create_dir_all(&fixture.config.state_dir).unwrap();

    let offline = MockServer::start().await;
    let mut config = fixture.config.clone();
    config.portal_url = offline.uri();

    let agent = Agent::new(config).unwrap();
    let outcome = agent.run_once().await.unwrap();

    assert_eq!(outcome.health, "degraded");
    assert!(!outcome.error.is_empty());
}

#[tokio::test]
async fn no_active_subscriptions_keeps_cache() {
    let fixture = fixture("s_auto").await;
    std::fs::create_dir_all(&fixture.config.state_dir).unwrap();

    fixture.portal.reset().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/nodes/home/state"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "revision": "none",
            "selectionRevision": "sha256:sel",
            "selection": "s_auto",
            "outbounds": []
        })))
        .mount(&fixture.portal)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/nodes/home/status"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&fixture.portal)
        .await;

    let previous = json!([{ "type": "direct", "tag": "old" }]);
    std::fs::write(
        fixture.config.outbounds_path(),
        serde_json::to_vec(&previous).unwrap(),
    )
    .unwrap();

    let agent = Agent::new(fixture.config.clone()).unwrap();
    let outcome = agent.run_once().await.unwrap();

    assert_eq!(outcome.health, "no_active_subscriptions");
    let cached: Value =
        serde_json::from_slice(&std::fs::read(fixture.config.outbounds_path()).unwrap()).unwrap();
    assert_eq!(cached, previous);
}

#[allow(dead_code)]
fn type_check() {
    let _: Option<PathBuf> = None;
}
