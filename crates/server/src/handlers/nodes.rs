use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};
use sing_portal_core::render::AUTO_TAG;
use sing_portal_store::SelectionMode;

use crate::error::{ApiError, ApiResult};
use crate::handlers::views::{
    MeasurementView, NodeView, ReportView, SelectionResponse, TestResponse,
};
use crate::service;
use crate::state::AppState;

pub async fn list(State(state): State<AppState>) -> ApiResult<Json<Vec<NodeView>>> {
    let machines = service::known_machines(&state).await?;
    let mut views = Vec::with_capacity(machines.len());

    for name in machines {
        let selection = state.store().selection_with_mode(&name).await?;
        let (outbound, mode) = match selection {
            Some(selection) => (selection.outbound, selection.mode.as_str().to_string()),
            None => (AUTO_TAG.to_string(), SelectionMode::Auto.as_str().to_string()),
        };

        let report = state.store().report(&name).await?;
        let measurements = state.store().measurements(&name).await?;
        let best = measurements.first().map(|row| row.tag.clone());
        let best_score = measurements.first().map(|row| row.score);
        let score = measurements
            .iter()
            .find(|row| row.tag == outbound)
            .map(|row| row.score);
        let measured_at = measurements.first().map(|row| row.measured_at);

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
) -> ApiResult<Json<Vec<MeasurementView>>> {
    if !service::is_known_machine(&state, &name).await {
        return Err(ApiError::NotFound(format!("unknown machine {name}")));
    }

    let rows = state.store().measurements(&name).await?;
    let best = rows.first().map(|row| row.tag.clone());

    Ok(Json(
        rows.into_iter()
            .map(|row| MeasurementView {
                best: best.as_deref() == Some(row.tag.as_str()),
                tag: row.tag,
                latency_ms: row.latency_ms,
                down_mbps: row.down_mbps,
                score: row.score,
                measured_at: row.measured_at,
            })
            .collect(),
    ))
}

async fn spawn_evaluation(state: &AppState, name: &str) -> Value {
    if !state.begin_auto(name).await {
        return json!({ "status": "already running" });
    }

    let state = state.clone();
    let machine = name.to_string();
    tokio::spawn(async move {
        match crate::auto::evaluate_machine(&state, &machine).await {
            Ok(Some(evaluation)) => tracing::info!(
                machine = %machine,
                switched = evaluation.switched,
                best = ?evaluation.best,
                score = ?evaluation.score,
                "auto selection evaluated"
            ),
            Ok(None) => {}
            Err(error) => tracing::warn!(machine = %machine, %error, "auto selection failed"),
        }
        state.end_auto(&machine).await;
    });

    json!({ "status": "started" })
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

    Ok(Json(
        service::probe(&state, &name, &request.tag, kind).await?,
    ))
}
