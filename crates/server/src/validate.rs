use std::io::Write;

use serde_json::Value;
use sing_portal_core::render::{config, to_bytes};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub async fn check(state: &AppState, outbounds: &[Value]) -> ApiResult<bool> {
    let Some(binary) = state.config().sing_box.as_ref() else {
        return Ok(false);
    };

    let config = config(outbounds);

    let mut file = tempfile::Builder::new()
        .prefix("sing-portal-check-")
        .suffix(".json")
        .tempfile_in(temp_dir(state))
        .map_err(|error| ApiError::Internal(format!("cannot create temp file: {error}")))?;
    file.write_all(&to_bytes(&config))
        .and_then(|()| file.flush())
        .map_err(|error| ApiError::Internal(format!("cannot write temp file: {error}")))?;

    let output = tokio::process::Command::new(binary)
        .arg("check")
        .arg("-c")
        .arg(file.path())
        .output()
        .await
        .map_err(|error| ApiError::Internal(format!("cannot run sing-box check: {error}")))?;

    if output.status.success() {
        return Ok(true);
    }

    Err(ApiError::Unprocessable(format!(
        "sing-box check rejected the subscription: {}",
        summarize(&String::from_utf8_lossy(&output.stderr))
    )))
}

fn summarize(stderr: &str) -> String {
    let joined: String = stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(4)
        .collect::<Vec<_>>()
        .join("; ");
    let summarized: String = joined.chars().take(500).collect();
    if summarized.is_empty() {
        "no error output".to_string()
    } else {
        summarized
    }
}

fn temp_dir(state: &AppState) -> std::path::PathBuf {
    state
        .config()
        .db_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir)
}
