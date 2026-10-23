use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::Value;
use sing_portal_core::model::outbound_tag;
use sing_portal_core::render::{AUTO_TAG, DIRECT_TAG, SELECTOR_TAG, config, to_bytes};

use crate::api::BoxApi;
use crate::config::AgentConfig;
use crate::error::{AgentError, Result};
use crate::state;

pub async fn apply(config: &AgentConfig, outbounds: &[Value], api: &BoxApi) -> Result<()> {
    let bytes = to_bytes(&config_of(outbounds));

    let next = config.config_next_path();
    state::write_atomic(&next, &bytes).await?;
    check(config, &next).await?;

    let outbounds_path = config.outbounds_path();
    if tokio::fs::try_exists(&outbounds_path).await.unwrap_or(false) {
        tokio::fs::copy(&outbounds_path, config.outbounds_previous_path()).await?;
    }

    state::write_atomic(&outbounds_path, &serde_json::to_vec_pretty(outbounds)?).await?;
    state::write_atomic(&config.config_path, &bytes).await?;
    let _ = tokio::fs::remove_file(&next).await;

    restart(config).await?;

    match wait_healthy(config, api).await {
        Ok(()) => Ok(()),
        Err(error) => {
            rollback(config, api).await?;
            Err(error)
        }
    }
}

pub async fn check(config: &AgentConfig, path: &Path) -> Result<()> {
    let output = tokio::process::Command::new(&config.sing_box_bin)
        .arg("check")
        .arg("-c")
        .arg(path)
        .output()
        .await?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(AgentError::Check(stderr.trim().to_string()))
}

async fn restart(config: &AgentConfig) -> Result<()> {
    let Some(command) = config.restart_cmd.as_deref().filter(|command| !command.is_empty()) else {
        return Ok(());
    };

    let output = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .output()
        .await?;

    if !output.status.success() {
        return Err(AgentError::Portal(format!(
            "restart command failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

async fn wait_healthy(config: &AgentConfig, api: &BoxApi) -> Result<()> {
    let deadline = Instant::now() + config.health_timeout;
    loop {
        if api.version().await.is_ok() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(AgentError::Unhealthy);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn rollback(config: &AgentConfig, api: &BoxApi) -> Result<()> {
    let previous = config.outbounds_previous_path();
    if !tokio::fs::try_exists(&previous).await.unwrap_or(false) {
        return Err(AgentError::Check(
            "no previous outbounds to roll back to".into(),
        ));
    }

    let outbounds: Value = serde_json::from_slice(&tokio::fs::read(&previous).await?)?;
    let outbounds = outbounds.as_array().cloned().unwrap_or_default();
    state::write_atomic(
        &config.outbounds_path(),
        &serde_json::to_vec_pretty(&outbounds)?,
    )
    .await?;
    state::write_atomic(&config.config_path, &to_bytes(&config_of(&outbounds))).await?;
    restart(config).await?;
    wait_healthy(config, api).await
}

fn config_of(outbounds: &[Value]) -> Value {
    config(outbounds)
}

pub async fn apply_selection(
    api: &BoxApi,
    desired: &str,
    tags: &[String],
) -> Result<String> {
    let valid = |candidate: &str| {
        matches!(candidate, AUTO_TAG | DIRECT_TAG)
            || tags.iter().any(|tag| tag == candidate)
    };
    let target = if valid(desired) {
        desired.to_string()
    } else {
        AUTO_TAG.to_string()
    };

    if let Ok(current) = api.current_selection().await
        && current == target
    {
        return Ok(target);
    }

    match api.select(SELECTOR_TAG, &target).await {
        Ok(()) => Ok(target),
        Err(error) => {
            if target != AUTO_TAG && api.select(SELECTOR_TAG, AUTO_TAG).await.is_ok() {
                Ok(AUTO_TAG.to_string())
            } else {
                Err(error)
            }
        }
    }
}

pub fn tags_of(outbounds: &[Value]) -> Vec<String> {
    outbounds
        .iter()
        .filter_map(outbound_tag)
        .map(ToString::to_string)
        .collect()
}
