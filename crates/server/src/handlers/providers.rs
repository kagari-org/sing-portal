use axum::Json;
use axum::extract::State;
use serde_json::Value;

use crate::error::ApiResult;
use crate::handlers::views::{ProviderNodeView, ProviderView};
use crate::state::AppState;

pub async fn list(State(state): State<AppState>) -> ApiResult<Json<Vec<ProviderView>>> {
    let subscriptions = state.store().list_subscriptions().await?;

    let providers = subscriptions
        .iter()
        .map(|subscription| ProviderView {
            id: subscription.id.clone(),
            name: subscription.name.clone(),
            slug: subscription.slug.clone(),
            format: subscription.format.clone(),
            enabled: subscription.enabled,
            last_error: subscription.last_error.clone(),
            last_fetched_at: subscription.last_fetched_at,
            nodes: subscription
                .outbounds
                .iter()
                .map(provider_node)
                .collect(),
        })
        .collect();

    Ok(Json(providers))
}

fn provider_node(outbound: &Value) -> ProviderNodeView {
    ProviderNodeView {
        tag: outbound
            .get("tag")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        kind: outbound
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        server: outbound
            .get("server")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        server_port: outbound.get("server_port").and_then(Value::as_u64),
    }
}
