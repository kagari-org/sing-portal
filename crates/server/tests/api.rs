use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sing_portal_core::protocol::{AgentMessage, ServerMessage};
use sing_portal_server::{AppState, ServerConfig, router};
use sing_portal_store::Measurement;
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
    state
        .store()
        .seed_selection("home", "s_auto")
        .await
        .unwrap();

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
                .body(Body::from(
                    json!({ "outbound": "provider-a · US 01" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::CONFLICT,
        "auto machines must be switched to manual before picking an egress"
    );

    let response = router(app.state.clone())
        .oneshot(
            request("PUT", "/api/nodes/home/mode")
                .body(Body::from(json!({ "mode": "manual" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

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
                .body(Body::from(
                    json!({ "outbound": "provider-a · US 01" }).to_string(),
                ))
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

#[tokio::test]
async fn disabling_a_node_removes_it_from_the_active_config() {
    let app = app().await;
    create_subscription(&app, "Provider A").await;

    let body = get_json(&app, "/api/providers").await;
    assert_eq!(body[0]["nodes"][0]["enabled"], true);

    let response = router(app.state.clone())
        .oneshot(
            request("PUT", "/api/nodes/enabled")
                .body(Body::from(
                    json!({ "tag": "provider-a · US 01", "enabled": false }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let providers = get_json(&app, "/api/providers").await;
    let disabled = providers[0]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["tag"] == "provider-a · US 01")
        .unwrap();
    assert_eq!(disabled["enabled"], false);

    let revision = get_json(&app, "/api/revision").await;
    assert_eq!(revision["outbounds"], 1);

    let response = router(app.state.clone())
        .oneshot(
            request("POST", "/api/nodes/home/test")
                .body(Body::from(
                    json!({ "tag": "provider-a · US 01" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let response = router(app.state.clone())
        .oneshot(
            request("PUT", "/api/nodes/enabled")
                .body(Body::from(
                    json!({ "tag": "provider-a · US 01", "enabled": true }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let revision = get_json(&app, "/api/revision").await;
    assert_eq!(revision["outbounds"], 2);
}

#[tokio::test]
async fn measurements_keep_automatic_and_manual_results_apart() {
    let app = app().await;
    create_subscription(&app, "Provider A").await;

    app.state
        .store()
        .replace_measurements(
            "home",
            &[Measurement {
                node_name: "home".into(),
                tag: "provider-a · US 01".into(),
                latency_ms: Some(30.0),
                down_mbps: Some(10.0),
                score: 91.0,
                measured_at: 100,
            }],
        )
        .await
        .unwrap();
    app.state
        .store()
        .record_manual_measurement("home", "provider-a · US 01", Some(24.0), Some(40.0), 200)
        .await
        .unwrap();

    let body = get_json(&app, "/api/nodes/home/measurements").await;
    assert_eq!(body["auto"].as_array().unwrap().len(), 1);
    assert_eq!(body["auto"][0]["latencyMs"], 30.0);
    assert_eq!(body["auto"][0]["downMbps"], 10.0);
    assert_eq!(body["auto"][0]["best"], true);
    assert_eq!(body["manual"].as_array().unwrap().len(), 1);
    assert_eq!(body["manual"][0]["latencyMs"], 24.0);
    assert_eq!(body["manual"][0]["downMbps"], 40.0);
}

#[tokio::test]
async fn evaluation_progress_is_exposed() {
    let app = app().await;
    create_subscription(&app, "Provider A").await;

    let body = get_json(&app, "/api/nodes/home/evaluation").await;
    assert_eq!(body, Value::Null);

    assert!(app.state.begin_auto("home").await);
    let body = get_json(&app, "/api/nodes/home/evaluation").await;
    assert_eq!(body["running"], true);
    assert_eq!(body["phase"], "starting");
    assert_eq!(body["machine"], "home");

    app.state
        .update_evaluation("home", "latency", 3, 10, Some("provider-a · US 01"))
        .await;
    let body = get_json(&app, "/api/nodes/home/evaluation").await;
    assert_eq!(body["phase"], "latency");
    assert_eq!(body["done"], 3);
    assert_eq!(body["total"], 10);
    assert_eq!(body["current"], "provider-a · US 01");

    app.state
        .finish_evaluation(
            "home",
            Some("provider-a · US 01".into()),
            Some(91.0),
            true,
            1,
            "",
        )
        .await;
    app.state.end_auto("home").await;

    let body = get_json(&app, "/api/nodes/home/evaluation").await;
    assert_eq!(body["running"], false);
    assert_eq!(body["phase"], "done");
    assert_eq!(body["best"], "provider-a · US 01");
    assert_eq!(body["switched"], true);
}

#[tokio::test]
async fn evaluation_skips_a_candidate_that_fails_the_egress_check() {
    let app = app().await;
    create_subscription(&app, "Provider A").await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let state = app.state.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, router(state)).await.unwrap();
    });

    let selects = Arc::new(Mutex::new(Vec::<String>::new()));
    let agent_selects = selects.clone();

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

    let agent = tokio::spawn(async move {
        while let Some(frame) = socket.next().await {
            let Ok(Message::Text(text)) = frame else {
                break;
            };
            let Ok(message) = serde_json::from_str::<ServerMessage>(&text) else {
                continue;
            };

            let reply = match message {
                ServerMessage::Sync {
                    id,
                    revision,
                    selection,
                    ..
                } => AgentMessage::SyncResult {
                    id,
                    revision,
                    selection,
                    health: "ok".into(),
                    error: String::new(),
                    applied: false,
                },
                ServerMessage::Select { id, outbound } => {
                    agent_selects.lock().unwrap().push(outbound);
                    AgentMessage::SelectResult { id, applied: true }
                }
                ServerMessage::Test { id, kind, tag } => {
                    if kind == "latency" {
                        let latency = if tag.contains("US 01") { 10.0 } else { 30.0 };
                        AgentMessage::TestResult {
                            id,
                            kind,
                            latency_ms: Some(latency),
                            down_mbps: None,
                            up_mbps: None,
                            responsiveness: None,
                        }
                    } else if kind == "both" {
                        let latency = if tag.contains("US 01") { 20.0 } else { 40.0 };
                        AgentMessage::TestResult {
                            id,
                            kind,
                            latency_ms: Some(latency),
                            down_mbps: Some(100.0),
                            up_mbps: None,
                            responsiveness: None,
                        }
                    } else if tag.contains("US 01") {
                        AgentMessage::Error {
                            id: Some(id),
                            message: "egress check failed: http 502".into(),
                        }
                    } else {
                        AgentMessage::TestResult {
                            id,
                            kind,
                            latency_ms: Some(50.0),
                            down_mbps: None,
                            up_mbps: None,
                            responsiveness: None,
                        }
                    }
                }
            };

            if socket
                .send(Message::Text(serde_json::to_string(&reply).unwrap().into()))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    for _ in 0..100 {
        if app.state.registry().is_connected("home").await {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let response = router(app.state.clone())
        .oneshot(
            request("POST", "/api/nodes/home/auto")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);

    let mut selection = None;
    for _ in 0..200 {
        selection = app.state.store().selection("home").await.unwrap();
        if selection.as_deref() == Some("provider-a · JP 02") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        selection.as_deref(),
        Some("provider-a · JP 02"),
        "the highest score failed its egress check, so the next candidate wins"
    );

    let mut evaluation = app.state.evaluation("home").await.unwrap();
    for _ in 0..100 {
        if !evaluation.running {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
        evaluation = app.state.evaluation("home").await.unwrap();
    }
    assert!(!evaluation.running);
    assert_eq!(evaluation.best.as_deref(), Some("provider-a · JP 02"));
    assert_eq!(evaluation.checked, 2);
    assert!(evaluation.error.is_empty());

    let mut recorded = Vec::new();
    for _ in 0..100 {
        recorded = selects.lock().unwrap().clone();
        if !recorded.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        recorded,
        vec!["provider-a · JP 02".to_string()],
        "candidate checks must go through s_test; only the final winner switches s_out"
    );

    agent.abort();
    server.abort();
}

#[tokio::test]
async fn degraded_egress_status_starts_a_recovery_evaluation() {
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

    for _ in 0..100 {
        if app.state.registry().is_connected("home").await {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    socket
        .send(Message::Text(
            serde_json::to_string(&AgentMessage::Status {
                revision: String::new(),
                selection: "s_auto".into(),
                health: "egress_degraded".into(),
                error: "real egress check failed".into(),
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();

    let mut started = false;
    for _ in 0..100 {
        if let Some(evaluation) = app.state.evaluation("home").await
            && evaluation.running
        {
            started = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(started, "watchdog should start a recovery evaluation");

    server.abort();
}
