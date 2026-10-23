use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sing_portal_core::protocol::{AgentMessage, ServerMessage};
use sing_portal_server::{AppState, ServerConfig, router};
use tempfile::TempDir;
use tokio_tungstenite::tungstenite::Message;
use tower::ServiceExt;

struct TestApp {
    _dir: TempDir,
    state: AppState,
}

async fn app() -> TestApp {
    let dir = TempDir::new().unwrap();
    let static_dir = dir.path().join("web");
    std::fs::create_dir_all(&static_dir).unwrap();
    std::fs::write(static_dir.join("index.html"), "<html>portal</html>").unwrap();

    let config = ServerConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        db_path: dir.path().join("portal.db"),
        sing_box: None,
        fetch_proxy: None,
        refresh_default: Duration::from_secs(3600),
        fetch_timeout: Duration::from_secs(5),
        fetch_max_bytes: 1024 * 1024,
        static_dir: Some(static_dir),
        embed_web: false,
        auto_interval: Duration::from_secs(600),
        auto_candidates: 5,
        auto_margin: 0.15,
        score: sing_portal_core::score::ScoreConfig::default(),
    };

    let state = AppState::new(config).await.unwrap();
    state.store().seed_selection("home", "s_auto").await.unwrap();

    TestApp { _dir: dir, state }
}

fn request(method: &str, path: &str) -> axum::http::request::Builder {
    Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-portal-request", "1")
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

async fn create_subscription(app: &TestApp, name: &str) -> Value {
    let content = include_str!("../../../fixtures/sub-singbox.json");
    let payload = json!({ "name": name, "content": content });

    let response = router(app.state.clone())
        .oneshot(
            request("POST", "/api/subscriptions")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
    let body = body_json(response).await;
    body["subscription"].clone()
}

async fn get_json(app: &TestApp, path: &str) -> Value {
    let response = router(app.state.clone())
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    body_json(response).await
}

#[tokio::test]
async fn health_endpoint_reports_ok() {
    let app = app().await;
    let response = router(app.state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await["status"], "ok");
}

#[tokio::test]
async fn creates_subscription_from_pasted_content() {
    let app = app().await;
    let subscription = create_subscription(&app, "Provider A").await;

    assert_eq!(subscription["slug"], "provider-a");
    assert_eq!(subscription["format"], "sing-box-json");
    assert_eq!(subscription["outboundCount"], 2);
    assert_eq!(subscription["source"], "manual");
    assert_eq!(subscription["validated"], false);
    assert_eq!(subscription["url"], Value::Null);
}

#[tokio::test]
async fn writes_require_portal_header() {
    let app = app().await;
    let payload = json!({ "name": "x", "content": "vmess://abc" });

    let response = router(app.state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/subscriptions")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn rejects_invalid_subscription_content() {
    let app = app().await;
    let response = router(app.state.clone())
        .oneshot(
            request("POST", "/api/subscriptions")
                .body(Body::from(
                    json!({ "name": "broken", "content": "<html>expired</html>" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        app.state.store().list_subscriptions().await.unwrap().len(),
        0
    );
}

#[tokio::test]
async fn providers_group_nodes_by_subscription() {
    let app = app().await;
    create_subscription(&app, "Provider A").await;

    let providers = get_json(&app, "/api/providers").await;
    let provider = &providers[0];
    assert_eq!(provider["name"], "Provider A");
    assert_eq!(provider["slug"], "provider-a");
    assert_eq!(provider["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(provider["nodes"][0]["kind"], "hysteria2");
    assert_eq!(provider["nodes"][0]["server"], "jp.example.com");
}

#[tokio::test]
async fn selection_validation_and_queueing() {
    let app = app().await;
    create_subscription(&app, "Provider A").await;

    let response = router(app.state.clone())
        .oneshot(
            request("PUT", "/api/nodes/home/selection")
                .body(Body::from(json!({ "outbound": "nope" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let response = router(app.state.clone())
        .oneshot(
            request("PUT", "/api/nodes/home/selection")
                .body(Body::from(json!({ "outbound": "provider-a · US 01" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(body_json(response).await["applied"], false);

    assert_eq!(
        app.state.store().selection("home").await.unwrap(),
        Some("provider-a · US 01".to_string())
    );
}

#[tokio::test]
async fn selection_mode_can_be_toggled() {
    let app = app().await;

    let nodes = get_json(&app, "/api/nodes").await;
    assert_eq!(nodes[0]["mode"], "auto");
    assert_eq!(nodes[0]["reachable"], false);

    let response = router(app.state.clone())
        .oneshot(
            request("PUT", "/api/nodes/home/mode")
                .body(Body::from(json!({ "mode": "manual" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let nodes = get_json(&app, "/api/nodes").await;
    assert_eq!(nodes[0]["mode"], "manual");

    let response = router(app.state.clone())
        .oneshot(
            request("PUT", "/api/nodes/home/mode")
                .body(Body::from(json!({ "mode": "sometimes" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn deleting_subscription_resets_selection() {
    let app = app().await;
    let subscription = create_subscription(&app, "Provider A").await;
    let id = subscription["id"].as_str().unwrap().to_string();

    app.state
        .store()
        .set_selection("home", "provider-a · US 01")
        .await
        .unwrap();

    let response = router(app.state.clone())
        .oneshot(
            request("DELETE", &format!("/api/subscriptions/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    assert_eq!(
        app.state.store().selection("home").await.unwrap(),
        Some("s_auto".to_string())
    );
}

#[tokio::test]
async fn export_import_roundtrip() {
    let app = app().await;
    create_subscription(&app, "Provider A").await;

    let response = router(app.state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/export")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let snapshot = body_json(response).await;
    assert_eq!(snapshot["version"], 1);
    assert_eq!(snapshot["subscriptions"].as_array().unwrap().len(), 1);

    let response = router(app.state.clone())
        .oneshot(
            request("POST", "/api/import")
                .body(Body::from(snapshot.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        app.state.store().list_subscriptions().await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn revision_endpoint_counts_active_outbounds() {
    let app = app().await;
    create_subscription(&app, "Provider A").await;

    let body = get_json(&app, "/api/revision").await;
    assert_eq!(body["outbounds"], 2);
    assert_eq!(body["enabledSubscriptions"], 1);
}

#[tokio::test]
async fn spa_fallback_serves_index() {
    let app = app().await;
    let response = router(app.state.clone())
        .oneshot(
            Request::builder()
                .uri("/proxies")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert!(String::from_utf8_lossy(&bytes).contains("portal"));
}

#[tokio::test]
async fn test_endpoint_validates_kind_and_tags() {
    let app = app().await;
    create_subscription(&app, "Provider A").await;

    let response = router(app.state.clone())
        .oneshot(
            request("POST", "/api/nodes/home/test")
                .body(Body::from(
                    json!({ "tag": "provider-a · US 01", "kind": "throughput" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = router(app.state.clone())
        .oneshot(
            request("POST", "/api/nodes/home/test")
                .body(Body::from(json!({ "tag": "missing" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn agent_over_websocket_receives_state_and_reports_status() {
    let app = app().await;
    create_subscription(&app, "Provider A").await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let state = app.state.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, router(state)).await.unwrap();
    });

    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("ws://{address}/api/v1/agent/ws"))
            .await
            .unwrap();

    socket
        .send(Message::Text(
            serde_json::to_string(&AgentMessage::Hello {
                name: "home".into(),
                version: "test".into(),
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();

    let frame = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let Message::Text(text) = frame else {
        panic!("expected text frame");
    };
    let ServerMessage::Sync {
        revision,
        selection,
        outbounds,
        ..
    } = serde_json::from_str(&text).unwrap()
    else {
        panic!("expected sync");
    };
    assert!(revision.starts_with("sha256:"));
    assert_eq!(selection, "s_auto");
    assert_eq!(outbounds.len(), 2);

    socket
        .send(Message::Text(
            serde_json::to_string(&AgentMessage::Status {
                revision: revision.clone(),
                selection: "s_auto".into(),
                health: "ok".into(),
                error: String::new(),
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();

    let mut report = None;
    for _ in 0..50 {
        report = app.state.store().report("home").await.unwrap();
        if report.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let report = report.expect("status stored");
    assert_eq!(report.revision, revision);
    assert_eq!(report.health, "ok");

    let nodes = get_json(&app, "/api/nodes").await;
    assert_eq!(nodes[0]["reachable"], true);
    assert_eq!(nodes[0]["version"], "test");

    server.abort();
}
