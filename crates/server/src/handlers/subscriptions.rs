use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use sing_portal_core::canonical::{slugify, unique_slug};
use sing_portal_store::{NewSubscription, Source, SubscriptionPatch};

use crate::config::parse_duration;
use crate::error::{ApiError, ApiResult};
use crate::handlers::views::{MutationView, SubscriptionView};
use crate::service;
use crate::state::AppState;

pub async fn list(State(state): State<AppState>) -> ApiResult<Json<Vec<SubscriptionView>>> {
    let subscriptions = state.store().list_subscriptions().await?;
    Ok(Json(
        subscriptions.iter().map(SubscriptionView::from).collect(),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRequest {
    pub name: String,
    pub url: Option<String>,
    pub content: Option<String>,
    pub user_agent: Option<String>,
    pub enabled: Option<bool>,
    pub auto_refresh: Option<bool>,
    pub refresh_interval: Option<String>,
}

pub async fn create(
    State(state): State<AppState>,
    Json(request): Json<CreateRequest>,
) -> ApiResult<(StatusCode, Json<MutationView>)> {
    let name = request.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest("name must not be empty".into()));
    }
    if request.url.is_some() == request.content.is_some() {
        return Err(ApiError::BadRequest(
            "provide exactly one of url or content".into(),
        ));
    }

    let taken = state.store().slugs().await?;
    let slug = unique_slug(&slugify(name), &taken);

    let interval = match &request.refresh_interval {
        Some(value) => parse_duration(value).map_err(ApiError::BadRequest)?,
        None => state.config().refresh_default,
    };

    let prepared = service::prepare(
        &state,
        &slug,
        request.user_agent.as_deref().unwrap_or_default(),
        request.url.as_deref(),
        request.content.as_deref(),
    )
    .await?;

    let now = sing_portal_store::now();
    let new = NewSubscription {
        id: uuid::Uuid::now_v7().to_string(),
        name: name.to_string(),
        slug: slug.clone(),
        source: if request.url.is_some() {
            Source::Url
        } else {
            Source::Manual
        },
        url: request.url.clone(),
        user_agent: request.user_agent.clone().unwrap_or_default(),
        format: prepared.format.as_str().to_string(),
        content: prepared.content.clone(),
        outbounds: prepared.outbounds.clone(),
        enabled: request.enabled.unwrap_or(true),
        auto_refresh: request.auto_refresh.unwrap_or(true),
        refresh_interval_secs: interval.as_secs() as i64,
        next_refresh_at: Some(now + interval.as_secs() as i64),
        failures: 0,
        now,
    };

    let created = state.store().create_subscription(&new).await?;
    state
        .store()
        .record_audit(
            "ui",
            "subscription.create",
            &slug,
            &format!(
                "format={} outbounds={}",
                prepared.format.as_str(),
                prepared.outbounds.len()
            ),
        )
        .await?;
    service::reconcile_selections(&state).await?;
    service::broadcast_state(&state).await;
    state.notify_changed(&["subscriptions", "providers", "revision", "nodes", "audit"]);

    Ok((
        StatusCode::CREATED,
        Json(MutationView {
            subscription: SubscriptionView::from(&created),
            warnings: prepared.warnings,
        }),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchRequest {
    pub name: Option<String>,
    pub url: Option<String>,
    pub user_agent: Option<String>,
    pub enabled: Option<bool>,
    pub auto_refresh: Option<bool>,
    pub refresh_interval: Option<String>,
}

pub async fn patch(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<PatchRequest>,
) -> ApiResult<Json<MutationView>> {
    let current = state
        .store()
        .get_subscription(&id)
        .await?
        .ok_or_else(|| ApiError::NotFound("subscription not found".into()))?;

    let interval_secs = match &request.refresh_interval {
        Some(value) => Some(
            parse_duration(value)
                .map_err(ApiError::BadRequest)?
                .as_secs() as i64,
        ),
        None => None,
    };
    let next_refresh_at = interval_secs.map(|secs| sing_portal_store::now().saturating_add(secs));

    let url_changed = match (&request.url, &current.url) {
        (Some(url), Some(existing)) => url != existing,
        (Some(_), None) => true,
        (None, _) => false,
    };

    if let Some(name) = &request.name
        && name.trim().is_empty()
    {
        return Err(ApiError::BadRequest("name must not be empty".into()));
    }

    let mut warnings = Vec::new();
    let mut updated = state
        .store()
        .update_subscription(
            &id,
            &SubscriptionPatch {
                name: request.name.as_ref().map(|name| name.trim().to_string()),
                url: request.url.clone(),
                user_agent: request.user_agent.clone(),
                enabled: request.enabled,
                auto_refresh: request.auto_refresh,
                refresh_interval_secs: interval_secs,
                next_refresh_at,
            },
        )
        .await?;

    if url_changed {
        let (refreshed, refresh_warnings) = service::refresh_subscription(&state, &updated).await?;
        updated = refreshed;
        warnings = refresh_warnings;
    }

    state
        .store()
        .record_audit(
            "ui",
            "subscription.update",
            &updated.slug,
            "settings changed",
        )
        .await?;
    service::reconcile_selections(&state).await?;
    service::broadcast_state(&state).await;
    state.notify_changed(&["subscriptions", "providers", "revision", "nodes", "audit"]);

    Ok(Json(MutationView {
        subscription: SubscriptionView::from(&updated),
        warnings,
    }))
}

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let subscription = state
        .store()
        .get_subscription(&id)
        .await?
        .ok_or_else(|| ApiError::NotFound("subscription not found".into()))?;

    state.store().delete_subscription(&id).await?;
    state
        .store()
        .record_audit("ui", "subscription.delete", &subscription.slug, "")
        .await?;
    service::reconcile_selections(&state).await?;
    service::broadcast_state(&state).await;
    state.notify_changed(&["subscriptions", "providers", "revision", "nodes", "audit"]);

    Ok(StatusCode::NO_CONTENT)
}

pub async fn refresh(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<MutationView>> {
    let subscription = state
        .store()
        .get_subscription(&id)
        .await?
        .ok_or_else(|| ApiError::NotFound("subscription not found".into()))?;

    if !state.begin_refresh(&id).await {
        return Err(ApiError::BadRequest(
            "subscription refresh already running".into(),
        ));
    }

    let result = service::refresh_subscription(&state, &subscription).await;
    state.end_refresh(&id).await;

    let (updated, warnings) = result?;
    state
        .store()
        .record_audit("ui", "subscription.refresh", &updated.slug, "")
        .await?;
    service::reconcile_selections(&state).await?;
    service::broadcast_state(&state).await;
    state.notify_changed(&["subscriptions", "providers", "revision", "nodes", "audit"]);

    Ok(Json(MutationView {
        subscription: SubscriptionView::from(&updated),
        warnings,
    }))
}
