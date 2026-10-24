use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};
use sing_portal_core::model::outbound_tag;
use sing_portal_core::render::AUTO_TAG;
use sing_portal_store::SelectionMode;

use crate::error::{ApiError, ApiResult};
use crate::evaluation::EvaluationProgress;
use crate::handlers::views::{
    ManualMeasurementView, MeasurementView, MeasurementsView, NodeView, ReportView,
    SelectionResponse, TestResponse,
};
use crate::service;
use crate::state::AppState;

pub async fn list(State(state): State<AppState>) -> ApiResult<Json<Vec<NodeView>>> {
    let machines = service::known_machines(&state).await?;
    let disabled = state.store().disabled_tags().await?;
    let mut views = Vec::with_capacity(machines.len());

    for name in machines {
        let selection = state.store().selection_with_mode(&name).await?;
        let (outbound, mode) = match selection {
            Some(selection) => (selection.outbound, selection.mode.as_str().to_string()),
            None => (
                AUTO_TAG.to_string(),
                SelectionMode::Auto.as_str().to_string(),
            ),
        };

        let report = state.store().report(&name).await?;
        let measurements = state.store().measurements(&name).await?;
        let best_row = measurements.iter().find(|row| !disabled.contains(&row.tag));
        let best = best_row.map(|row| row.tag.clone());
        let best_score = best_row.map(|row| row.score);
        let score = measurements
            .iter()
            .find(|row| row.tag == outbound)
            .map(|row| row.score);
        let measured_at = best_row.map(|row| row.measured_at);

        let reachable = state.registry().is_connected(&name).await;
        let version = state.registry().version(&name).await;

        let online = report
            .as_ref()
            .map(|report| sing_portal_store::now() - report.reported_at < 90)
            .unwrap_or(false);

        views.push(NodeView {
            name: name.clone(),
            address: if reachable {
                "agent connected".to_string()
            } else {
                "agent offline".to_string()
            },
            selection: outbound,
            mode,
            score,
            best,
            best_score,
            measured_at,
            reachable,
            version,
            report: report
                .as_ref()
                .map(|report| ReportView::new(report, online)),
            evaluation: state.evaluation(&name).await,
        });
    }

    Ok(Json(views))
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
    if !service::is_known_machine(&state, &name).await {
        return Err(ApiError::NotFound(format!("unknown machine {name}")));
    }

    if state
        .store()
        .selection_with_mode(&name)
        .await?
        .is_some_and(|selection| selection.mode == SelectionMode::Auto)
    {
        return Err(ApiError::Conflict(
            "selection is managed automatically; switch the machine to manual first".into(),
        ));
    }

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

    let applied = service::select(&state, &name, &request.outbound).await?;
    if let Err(error) = service::push_state(&state, &name).await {
        tracing::warn!(%name, %error, "cannot push selection to agent");
    }

    state
        .store()
        .record_audit(
            "ui",
            "selection.update",
            &name,
            &format!("outbound={} applied={applied}", request.outbound),
        )
        .await?;
    state.notify_changed(&["nodes", "audit"]);

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
pub struct ModeRequest {
    pub mode: String,
}

pub async fn set_mode(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(request): Json<ModeRequest>,
) -> ApiResult<Json<Value>> {
    if !service::is_known_machine(&state, &name).await {
        return Err(ApiError::NotFound(format!("unknown machine {name}")));
    }

    let mode = SelectionMode::parse(&request.mode).ok_or_else(|| {
        ApiError::BadRequest(format!("unsupported selection mode `{}`", request.mode))
    })?;

    state.store().set_selection_mode(&name, mode).await?;
    state
        .store()
        .record_audit("ui", "selection.mode", &name, mode.as_str())
        .await?;

    if let Err(error) = service::push_state(&state, &name).await {
        tracing::warn!(%name, %error, "cannot push selection mode to agent");
    }
    state.notify_changed(&["nodes", "audit"]);

    if mode == SelectionMode::Auto {
        spawn_evaluation(&state, &name).await;
    }

    Ok(Json(json!({ "mode": mode.as_str() })))
}

pub async fn auto(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    if !service::is_known_machine(&state, &name).await {
        return Err(ApiError::NotFound(format!("unknown machine {name}")));
    }

    let status = spawn_evaluation(&state, &name).await;
    Ok((StatusCode::ACCEPTED, Json(status)))
}

pub async fn measurements(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<MeasurementsView>> {
    if !service::is_known_machine(&state, &name).await {
        return Err(ApiError::NotFound(format!("unknown machine {name}")));
    }

    let rows = state.store().measurements(&name).await?;
    let disabled = state.store().disabled_tags().await?;
    let best = rows
        .iter()
        .find(|row| !disabled.contains(&row.tag))
        .map(|row| row.tag.clone());
    let auto = rows
        .into_iter()
        .map(|row| MeasurementView {
            best: best.as_deref() == Some(row.tag.as_str()),
            tag: row.tag,
            latency_ms: row.latency_ms,
            down_mbps: row.down_mbps,
            score: row.score,
            measured_at: row.measured_at,
        })
        .collect();
    let manual = state
        .store()
        .manual_measurements(&name)
        .await?
        .iter()
        .map(ManualMeasurementView::from)
        .collect();

    Ok(Json(MeasurementsView { auto, manual }))
}

pub async fn evaluation(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<Option<EvaluationProgress>>> {
    if !service::is_known_machine(&state, &name).await {
        return Err(ApiError::NotFound(format!("unknown machine {name}")));
    }

    Ok(Json(state.evaluation(&name).await))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnabledRequest {
    pub tag: String,
    pub enabled: bool,
}

pub async fn set_enabled(
    State(state): State<AppState>,
    Json(request): Json<EnabledRequest>,
) -> ApiResult<StatusCode> {
    let subscriptions = state.store().list_subscriptions().await?;
    let known = subscriptions.iter().any(|subscription| {
        subscription
            .outbounds
            .iter()
            .any(|outbound| outbound_tag(outbound) == Some(request.tag.as_str()))
    });
    if !known {
        return Err(ApiError::Unprocessable(format!(
            "unknown outbound `{}`",
            request.tag
        )));
    }

    state
        .store()
        .set_node_disabled(&request.tag, !request.enabled)
        .await?;

    if !request.enabled {
        service::reconcile_selections(&state).await?;
    }
    service::broadcast_state(&state).await;

    let action = if request.enabled {
        "node.enable"
    } else {
        "node.disable"
    };
    state
        .store()
        .record_audit("ui", action, &request.tag, "")
        .await?;
    state.notify_changed(&["providers", "revision", "nodes", "measurements", "audit"]);

    Ok(StatusCode::NO_CONTENT)
}

async fn spawn_evaluation(state: &AppState, name: &str) -> Value {
    if crate::auto::trigger(state, name).await {
        json!({ "status": "started" })
    } else {
        json!({ "status": "already running" })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestRequest {
    pub tag: String,
    pub kind: Option<String>,
}

pub async fn test(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(request): Json<TestRequest>,
) -> ApiResult<Json<TestResponse>> {
    if !service::is_known_machine(&state, &name).await {
        return Err(ApiError::NotFound(format!("unknown machine {name}")));
    }

    let tags = service::active_tags(&state).await?;
    if !tags.iter().any(|tag| tag == &request.tag) {
        return Err(ApiError::Unprocessable(format!(
            "unknown outbound `{}`",
            request.tag
        )));
    }

    let kind = request.kind.as_deref().unwrap_or("latency");
    if !matches!(kind, "latency" | "bandwidth" | "both") {
        return Err(ApiError::BadRequest(format!(
            "unsupported test kind `{kind}`"
        )));
    }

    let result = service::probe(&state, &name, &request.tag, kind).await?;
    state
        .store()
        .record_manual_measurement(
            &name,
            &result.tag,
            result.latency_ms,
            result.down_mbps,
            sing_portal_store::now(),
        )
        .await?;
    state.notify_changed(&["measurements"]);

    Ok(Json(result))
}
