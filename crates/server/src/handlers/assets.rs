use axum::body::Body;
use axum::extract::State;
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

use crate::state::AppState;

#[derive(RustEmbed)]
#[folder = "../../web/dist"]
struct Embedded;

pub async fn handler(State(state): State<AppState>, uri: Uri) -> Response {
    let requested = uri.path().trim_start_matches('/');
    let requested = if requested.is_empty() {
        "index.html"
    } else {
        requested
    };

    if let Some(response) = serve(&state, requested).await {
        return response;
    }

    match serve(&state, "index.html").await {
        Some(response) => response,
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

async fn serve(state: &AppState, path: &str) -> Option<Response> {
    if path.split('/').any(|segment| segment == ".." || segment == ".") {
        return None;
    }

    match state.config().static_dir.as_ref() {
        Some(directory) => {
            let bytes = tokio::fs::read(directory.join(path)).await.ok()?;
            Some(build(path, bytes))
        }
        None => {
            let asset = Embedded::get(path)?;
            Some(build(path, asset.data.into_owned()))
        }
    }
}

fn build(path: &str, bytes: Vec<u8>) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    (
        [(header::CONTENT_TYPE, mime.as_ref()), (header::CACHE_CONTROL, "no-store")],
        Body::from(bytes),
    )
        .into_response()
}
