use std::collections::HashSet;

use axum::Json;
use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use serde::Deserialize;
use serde_json::{Value, json};
use sing_portal_store::{AuditEntry, Snapshot};

use crate::error::{ApiError, ApiResult};
use crate::handlers::views::RevisionView;
use crate::{service, state::AppState};

pub async fn healthz(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    state.store().healthz().await?;
    Ok(Json(json!({ "status": "ok" })))
}

pub async fn revision(State(state): State<AppState>) -> ApiResult<Json<RevisionView>> {
    let outbounds = service::active_outbounds(&state).await?;
    let revision = sing_portal_core::canonical::revision(&outbounds);
    let subscriptions = state.store().list_subscriptions().await?;
    let enabled = subscriptions
        .iter()
        .filter(|subscription| subscription.enabled)
        .count();

    Ok(Json(RevisionView {
        revision,
        outbounds: outbounds.len(),
        enabled_subscriptions: enabled,
    }))
}

#[derive(Deserialize)]
pub struct AuditQuery {
    pub limit: Option<i64>,
}

pub async fn audit(
    State(state): State<AppState>,
    Query(query): Query<AuditQuery>,
) -> ApiResult<Json<Vec<AuditEntry>>> {
    let limit = query.limit.unwrap_or(100);
    Ok(Json(state.store().audit_entries(limit).await?))
}

pub async fn export(State(state): State<AppState>) -> ApiResult<Response> {
    let snapshot = state.store().snapshot().await?;
    let body = serde_json::to_vec_pretty(&snapshot)
        .map_err(|error| ApiError::Internal(error.to_string()))?;

    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"sing-portal-export.json\""),
    );
    Ok(response)
}

pub async fn import(
    State(state): State<AppState>,
    Json(snapshot): Json<Snapshot>,
) -> ApiResult<StatusCode> {
    if snapshot.version != 1 {
        return Err(ApiError::BadRequest(format!(
            "unsupported snapshot version {}",
            snapshot.version
        )));
    }

    let mut slugs = HashSet::new();
    for subscription in &snapshot.subscriptions {
        if subscription.slug.trim().is_empty() || subscription.name.trim().is_empty() {
            return Err(ApiError::BadRequest(
                "snapshot contains a subscription without slug or name".into(),
            ));
        }
        if !slugs.insert(subscription.slug.clone()) {
            return Err(ApiError::BadRequest(format!(
                "snapshot contains duplicate slug `{}`",
                subscription.slug
            )));
        }
        if subscription.outbounds.is_empty() {
            return Err(ApiError::BadRequest(format!(
                "snapshot subscription `{}` has no outbounds",
                subscription.slug
            )));
        }
    }

    state.store().import_snapshot(&snapshot).await?;
    service::reconcile_selections(&state).await?;
    service::broadcast_state(&state).await;
    state
        .store()
        .record_audit(
            "ui",
            "snapshot.import",
            "",
            &format!(
                "subscriptions={} selections={}",
                snapshot.subscriptions.len(),
                snapshot.selections.len()
            ),
        )
        .await?;

    Ok(StatusCode::NO_CONTENT)
}
