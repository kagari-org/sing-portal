pub mod config;
pub mod error;
pub mod fetch;
pub mod handlers;
pub mod scheduler;
pub mod service;
pub mod state;
pub mod validate;

pub use config::{NodeConfig, ServerConfig, mask_url, parse_duration, parse_nodes};
pub use state::AppState;

use axum::Json;
use axum::Router;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post, put};
use serde_json::json;
use tower_http::trace::TraceLayer;

use crate::handlers::{agent, api, assets, nodes, providers, subscriptions};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/healthz", get(api::healthz))
        .route("/api/revision", get(api::revision))
        .route("/api/audit", get(api::audit))
        .route("/api/export", get(api::export))
        .route("/api/import", post(api::import))
        .route(
            "/api/subscriptions",
            get(subscriptions::list).post(subscriptions::create),
        )
        .route(
            "/api/subscriptions/{id}",
            patch(subscriptions::patch).delete(subscriptions::delete),
        )
        .route(
            "/api/subscriptions/{id}/refresh",
            post(subscriptions::refresh),
        )
        .route("/api/providers", get(providers::list))
        .route("/api/nodes", get(nodes::list))
        .route("/api/nodes/{name}/proxies", get(nodes::proxies))
        .route("/api/nodes/{name}/selection", put(nodes::set_selection))
        .route("/api/nodes/{name}/test", post(nodes::test))
        .route("/api/v1/nodes/{name}/state", get(agent::state))
        .route("/api/v1/nodes/{name}/status", post(agent::status))
        .fallback(assets::handler)
        .layer(middleware::from_fn(write_guard))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

pub async fn serve(state: AppState) -> std::io::Result<()> {
    let listen = state.config().listen;
    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!(%listen, "sing-portal listening");
    axum::serve(listener, router(state)).await
}

async fn write_guard(request: Request, next: Next) -> Response {
    let method = request.method();
    if method == Method::GET || method == Method::HEAD || method == Method::OPTIONS {
        return next.run(request).await;
    }

    let has_header = request
        .headers()
        .get("x-portal-request")
        .and_then(|value| value.to_str().ok())
        == Some("1");

    if has_header {
        next.run(request).await
    } else {
        (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "missing X-Portal-Request header" })),
        )
            .into_response()
    }
}
