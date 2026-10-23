use std::path::PathBuf;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sing_portal_server::{AppState, NodeConfig, ServerConfig, router};
use tempfile::TempDir;
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
        nodes: vec![NodeConfig {
            name: "home".into(),
            address: "127.0.0.1:1".into(),
        }],
        sing_box: None,
        fetch_proxy: None,
        refresh_default: Duration::from_secs(3600),
        fetch_timeout: Duration::from_secs(5),
        fetch_max_bytes: 1024 * 1024,
        static_dir: Some(static_dir),
        embed_web: false,
        test_port: 3016,
    };

    let state = AppState::new(config).await.unwrap();
    sing_portal_server::service::init_nodes(&state).await.unwrap();

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
    let subscription = create_subscription(&app, "Jichang A").await;

    assert_eq!(subscription["slug"], "jichang-a");
    assert_eq!(subscription["format"], "sing-box-json");
    assert_eq!(subscription["outboundCount"], 2);
    assert_eq!(subscription["source"], "manual");
    assert_eq!(subscription["validated"], false);
    assert_eq!(subscription["url"], Value::Null);

    let response = router(app.state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/subscriptions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let list = body_json(response).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
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
    assert_eq!(app.state.store().list_subscriptions().await.unwrap().len(), 0);
}

#[tokio::test]
async fn node_state_supports_304() {
    let app = app().await;
    create_subscription(&app, "Jichang A").await;

    let response = router(app.state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/v1/nodes/home/state")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let state = body_json(response).await;
    assert!(state["revision"].as_str().unwrap().starts_with("sha256:"));
    assert_eq!(state["selection"], "s_auto");
    assert_eq!(state["outbounds"].as_array().unwrap().len(), 2);

    let revision = state["revision"].as_str().unwrap().to_string();
    let selection_revision = state["selectionRevision"].as_str().unwrap().to_string();
    let uri = format!(
        "/api/v1/nodes/home/state?revision={revision}&selectionRevision={selection_revision}"
    );

    let response = router(app.state.clone())
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);

    let response = router(app.state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/v1/nodes/unknown/state")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn disabled_subscription_publishes_none_revision() {
    let app = app().await;
    let subscription = create_subscription(&app, "Jichang A").await;
    let id = subscription["id"].as_str().unwrap();

    let response = router(app.state.clone())
        .oneshot(
            request("PATCH", &format!("/api/subscriptions/{id}"))
                .body(Body::from(json!({ "enabled": false }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = router(app.state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/v1/nodes/home/state")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let state = body_json(response).await;
    assert_eq!(state["revision"], "none");
    assert_eq!(state["outbounds"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn status_reports_are_stored() {
    let app = app().await;
    let payload = json!({
        "revision": "sha256:abc",
        "selection": "s_auto",
        "health": "ok",
        "error": ""
    });

    let response = router(app.state.clone())
        .oneshot(
            request("POST", "/api/v1/nodes/home/status")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let response = router(app.state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/nodes")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let nodes = body_json(response).await;
    let node = &nodes[0];
    assert_eq!(node["name"], "home");
    assert_eq!(node["report"]["revision"], "sha256:abc");
    assert_eq!(node["report"]["online"], true);
    assert_eq!(node["reachable"], false);
}

#[tokio::test]
async fn selection_validation_and_queueing() {
    let app = app().await;
    create_subscription(&app, "Jichang A").await;

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
                .body(Body::from(json!({ "outbound": "jichang-a · US 01" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body = body_json(response).await;
    assert_eq!(body["applied"], false);

    assert_eq!(
        app.state.store().selection("home").await.unwrap(),
        Some("jichang-a · US 01".to_string())
    );
}

#[tokio::test]
async fn deleting_subscription_resets_selection() {
    let app = app().await;
    let subscription = create_subscription(&app, "Jichang A").await;
    let id = subscription["id"].as_str().unwrap().to_string();

    app.state
        .store()
        .set_selection("home", "jichang-a · US 01")
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
    create_subscription(&app, "Jichang A").await;

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
    assert_eq!(app.state.store().list_subscriptions().await.unwrap().len(), 1);
}

#[tokio::test]
async fn revision_endpoint_counts_active_outbounds() {
    let app = app().await;
    create_subscription(&app, "Jichang A").await;

    let response = router(app.state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/revision")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_json(response).await;
    assert_eq!(body["outbounds"], 2);
    assert_eq!(body["enabledSubscriptions"], 1);
}

#[tokio::test]
async fn spa_fallback_serves_index() {
    let app = app().await;
    let response = router(app.state.clone())
        .oneshot(
            Request::builder()
                .uri("/nodes")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert!(String::from_utf8_lossy(&bytes).contains("portal"));
}

#[allow(dead_code)]
fn assert_send_sync() {
    fn check<T: Send + Sync>() {}
    check::<AppState>();
    let _: PathBuf = PathBuf::new();
}

#[tokio::test]
async fn providers_group_nodes_by_subscription() {
    let app = app().await;
    create_subscription(&app, "Provider A").await;

    let response = router(app.state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/providers")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let providers = body_json(response).await;
    let provider = &providers[0];
    assert_eq!(provider["name"], "Provider A");
    assert_eq!(provider["slug"], "provider-a");
    assert_eq!(provider["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(provider["nodes"][0]["kind"], "hysteria2");
    assert_eq!(provider["nodes"][0]["server"], "jp.example.com");
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
