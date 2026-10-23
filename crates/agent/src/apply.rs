use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::Value;
use crate::api::BoxApi;
use sing_portal_core::model::outbound_tag;
use sing_portal_core::render::{AUTO_TAG, DIRECT_TAG, SELECTOR_TAG, render, to_bytes};

use crate::config::AgentConfig;
use crate::error::{AgentError, Result};
use crate::state;

pub async fn apply(config: &AgentConfig, outbounds: &[Value], api: &BoxApi) -> Result<()> {
    let template_bytes = tokio::fs::read(&config.template_path).await?;
    let template: Value = serde_json::from_slice(&template_bytes)?;
    let rendered = render(&template, outbounds)?;
    let config_bytes = to_bytes(&rendered);
    let outbounds_bytes = serde_json::to_vec_pretty(outbounds)?;

    let next = config.config_next_path();
    state::write_atomic(&next, &config_bytes).await?;
    check(config, &next).await?;

    let outbounds_path = config.outbounds_path();
    if tokio::fs::try_exists(&outbounds_path).await.unwrap_or(false) {
        tokio::fs::copy(&outbounds_path, config.outbounds_previous_path()).await?;
    }

    state::write_atomic(&outbounds_path, &outbounds_bytes).await?;
    state::write_atomic(&config.config_path, &config_bytes).await?;
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

    let outbounds_bytes = tokio::fs::read(&previous).await?;
    let outbounds: Vec<Value> = serde_json::from_slice(&outbounds_bytes)?;
    let template_bytes = tokio::fs::read(&config.template_path).await?;
    let template: Value = serde_json::from_slice(&template_bytes)?;
    let config_bytes = to_bytes(&render(&template, &outbounds)?);

    state::write_atomic(&config.outbounds_path(), &outbounds_bytes).await?;
    state::write_atomic(&config.config_path, &config_bytes).await?;
    restart(config).await?;
    wait_healthy(config, api).await
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
                Err(AgentError::BoxApi(error.to_string()))
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
