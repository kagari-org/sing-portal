use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use sing_portal_core::canonical::selection_revision;
use sing_portal_core::render::AUTO_TAG;
use sing_portal_store::NodeReport;

use crate::error::{ApiError, ApiResult};
use crate::handlers::views::NodeStateView;
use crate::{service, state::AppState};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateQuery {
    pub revision: Option<String>,
    pub selection_revision: Option<String>,
}

pub async fn state(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<StateQuery>,
) -> ApiResult<Response> {
    state
        .node(&name)
        .ok_or_else(|| ApiError::NotFound(format!("unknown node {name}")))?;

    let outbounds = service::active_outbounds(&state).await?;
    let revision = sing_portal_core::canonical::revision(&outbounds);
    let selection = state
        .store()
        .selection(&name)
        .await?
        .unwrap_or_else(|| AUTO_TAG.to_string());
    let selection_revision = selection_revision(&name, &selection);

    let unchanged = query.revision.as_deref() == Some(revision.as_str())
        && query.selection_revision.as_deref() == Some(selection_revision.as_str());

    if unchanged {
        return Ok(StatusCode::NOT_MODIFIED.into_response());
    }

    Ok(Json(NodeStateView {
        revision,
        selection_revision,
        selection,
        outbounds,
    })
    .into_response())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusRequest {
    pub revision: String,
    pub selection: String,
    pub health: String,
    pub error: Option<String>,
}

pub async fn status(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(request): Json<StatusRequest>,
) -> ApiResult<StatusCode> {
    state
        .node(&name)
        .ok_or_else(|| ApiError::NotFound(format!("unknown node {name}")))?;

    service::record_report(
        &state,
        NodeReport {
            node_name: name,
            revision: request.revision,
            selection: request.selection,
            health: request.health,
            error: request.error.unwrap_or_default(),
            reported_at: sing_portal_store::now(),
        },
    )
    .await?;

    Ok(StatusCode::NO_CONTENT)
}
