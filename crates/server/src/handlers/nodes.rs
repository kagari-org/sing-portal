use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::Value;
use sing_portal_core::clash_api::ClashClient;
use sing_portal_core::model::DEFAULT_TEST_URL;
use sing_portal_core::render::{AUTO_TAG, SELECTOR_TAG};

use crate::error::{ApiError, ApiResult};
use crate::handlers::views::{NodeView, ReportView, SelectionResponse, TestResponse};
use crate::service;
use crate::state::AppState;

pub async fn list(State(state): State<AppState>) -> ApiResult<Json<Vec<NodeView>>> {
    let mut views = Vec::with_capacity(state.config().nodes.len());

    for node in &state.config().nodes {
        let selection = state
            .store()
            .selection(&node.name)
            .await?
            .unwrap_or_else(|| AUTO_TAG.to_string());
        let report = state.store().report(&node.name).await?;

        let client = ClashClient::new(&node.clash_base_url(), Duration::from_secs(2))
            .map_err(ApiError::from_core)?;
        let version = tokio::time::timeout(Duration::from_secs(1), client.version())
            .await
            .ok()
            .and_then(Result::ok);

        let online = report
            .as_ref()
            .map(|report| sing_portal_store::now() - report.reported_at < 90)
            .unwrap_or(false);

        views.push(NodeView {
            name: node.name.clone(),
            address: node.address.clone(),
            selection,
            reachable: version.is_some(),
            version,
            report: report
                .as_ref()
                .map(|report| ReportView::new(report, online)),
        });
    }

    Ok(Json(views))
}

pub async fn proxies(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<Value>> {
    let node = state
        .node(&name)
        .ok_or_else(|| ApiError::NotFound(format!("unknown node {name}")))?;
    let client = ClashClient::new(&node.clash_base_url(), Duration::from_secs(5))
        .map_err(ApiError::from_core)?;
    let proxies = client.proxies().await.map_err(ApiError::from_core)?;
    Ok(Json(proxies))
}

#[derive(Deserialize)]
pub struct SelectionRequest {
    pub outbound: String,
}

pub async fn set_selection(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(request): Json<SelectionRequest>,
) -> ApiResult<(StatusCode, Json<SelectionResponse>)> {
    let node = state
        .node(&name)
        .ok_or_else(|| ApiError::NotFound(format!("unknown node {name}")))?;

    let tags = service::active_tags(&state).await?;
    if !service::is_valid_choice(&request.outbound, &tags) {
        return Err(ApiError::Unprocessable(format!(
            "unknown outbound `{}`",
            request.outbound
        )));
    }

    state
        .store()
        .set_selection(&name, &request.outbound)
        .await?;

    let client = ClashClient::new(&node.clash_base_url(), Duration::from_secs(3))
        .map_err(ApiError::from_core)?;
    let applied = client
        .select(SELECTOR_TAG, &request.outbound)
        .await
        .map(|()| true)
        .unwrap_or_else(|error| {
            tracing::warn!(node = %name, error = %error, "failed to apply selection, kept as desired value");
            false
        });

    state
        .store()
        .record_audit(
            "ui",
            "selection.update",
            &name,
            &format!("outbound={} applied={applied}", request.outbound),
        )
        .await?;

    let status = if applied {
        StatusCode::OK
    } else {
        StatusCode::ACCEPTED
    };

    Ok((
        status,
        Json(SelectionResponse {
            selection: request.outbound,
            applied,
        }),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestRequest {
    pub tag: String,
    pub kind: Option<String>,
    pub url: Option<String>,
}

pub async fn test(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(request): Json<TestRequest>,
) -> ApiResult<Json<TestResponse>> {
    let node = state
        .node(&name)
        .ok_or_else(|| ApiError::NotFound(format!("unknown node {name}")))?;

    let tags = service::active_tags(&state).await?;
    if !tags.iter().any(|tag| tag == &request.tag) {
        return Err(ApiError::Unprocessable(format!(
            "unknown outbound `{}`",
            request.tag
        )));
    }

    let kind = request.kind.as_deref().unwrap_or("latency");
    if !matches!(kind, "latency" | "bandwidth") {
        return Err(ApiError::BadRequest(format!(
            "unsupported test kind `{kind}`"
        )));
    }

    let agent_url = format!(
        "http://{}:{}/test/{}",
        node.host(),
        state.config().test_port,
        kind
    );
    let payload = serde_json::json!({ "tag": request.tag, "url": request.url });

    match state
        .test_http()
        .post(&agent_url)
        .header("x-portal-request", "1")
        .json(&payload)
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => {
            let body: Value = response.json().await.map_err(|error| {
                ApiError::Upstream(format!("invalid node test response: {error}"))
            })?;
            Ok(Json(TestResponse {
                tag: request.tag,
                kind: kind.to_string(),
                latency_ms: body.get("latencyMs").and_then(Value::as_u64),
                bytes: body.get("bytes").and_then(Value::as_u64),
                seconds: body.get("seconds").and_then(Value::as_f64),
                down_mbps: body.get("downMbps").and_then(Value::as_f64),
                source: "agent".to_string(),
                restored: body
                    .get("restored")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
            }))
        }
        Ok(response) => {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            fallback(
                node,
                &request.tag,
                kind,
                format!("node test endpoint returned {status}: {text}"),
            )
            .await
        }
        Err(error) => {
            fallback(
                node,
                &request.tag,
                kind,
                format!("node test endpoint unavailable: {error}"),
            )
            .await
        }
    }
}

async fn fallback(
    node: &crate::config::NodeConfig,
    tag: &str,
    kind: &str,
    reason: String,
) -> ApiResult<Json<TestResponse>> {
    if kind != "latency" {
        return Err(ApiError::Upstream(reason));
    }

    let client = ClashClient::new(&node.clash_base_url(), Duration::from_secs(10))
        .map_err(ApiError::from_core)?;
    let delay = client
        .delay(tag, DEFAULT_TEST_URL, 5000)
        .await
        .map_err(ApiError::from_core)?;

    Ok(Json(TestResponse {
        tag: tag.to_string(),
        kind: "latency".to_string(),
        latency_ms: Some(delay),
        bytes: None,
        seconds: None,
        down_mbps: None,
        source: "clash".to_string(),
        restored: None,
    }))
}
