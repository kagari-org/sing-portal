use axum::http::header::CONTENT_TYPE;
use reqwest::header::USER_AGENT;
use url::Url;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub const DEFAULT_USER_AGENT: &str = "sing-box/1.14.0";

pub async fn fetch(state: &AppState, url: &str, user_agent: &str) -> ApiResult<Vec<u8>> {
    let parsed = Url::parse(url)
        .map_err(|_| ApiError::BadRequest("invalid subscription url".into()))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(ApiError::BadRequest(
            "subscription url must use http or https".into(),
        ));
    }

    let user_agent = if user_agent.is_empty() {
        DEFAULT_USER_AGENT
    } else {
        user_agent
    };

    match request(
        state.http(),
        url,
        user_agent,
        state.config().fetch_max_bytes,
    )
    .await
    {
        Ok(body) => Ok(body),
        Err(error) => match state.proxy_http() {
            Some(client) => {
                tracing::warn!(error = %error, "direct fetch failed, retrying through proxy");
                request(client, url, user_agent, state.config().fetch_max_bytes).await
            }
            None => Err(error),
        },
    }
}

async fn request(
    client: &reqwest::Client,
    url: &str,
    user_agent: &str,
    max_bytes: usize,
) -> ApiResult<Vec<u8>> {
    let response = client
        .get(url)
        .header(USER_AGENT, user_agent)
        .send()
        .await
        .map_err(|error| ApiError::Upstream(format!("subscription fetch failed: {error}")))?;

    let status = response.status();
    if !status.is_success() {
        return Err(ApiError::Upstream(format!(
            "subscription endpoint returned HTTP {status}"
        )));
    }

    if let Some(length) = response.content_length()
        && length as usize > max_bytes
    {
        return Err(ApiError::Unprocessable(format!(
            "subscription is too large ({length} bytes, limit {max_bytes})"
        )));
    }

    let is_html = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/html"));

    let body = response
        .bytes()
        .await
        .map_err(|error| ApiError::Upstream(format!("subscription fetch failed: {error}")))?;

    if body.len() > max_bytes {
        return Err(ApiError::Unprocessable(format!(
            "subscription is too large ({} bytes, limit {max_bytes})",
            body.len()
        )));
    }
    if is_html {
        return Err(ApiError::Unprocessable(
            "subscription looks like an HTML page; the link is probably expired or requires a login"
                .into(),
        ));
    }

    Ok(body.to_vec())
}
