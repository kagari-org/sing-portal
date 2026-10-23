use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sing_portal_core::model::{is_proxy_outbound, outbound_tag};
use sing_portal_core::render::{
    AUTO_TAG, DIRECT_TAG, SELECTOR_TAG, TEST_INBOUND_TAG, TEST_SELECTOR, render, to_bytes,
};

use crate::api::BoxApi;
use crate::config::AgentConfig;
use crate::error::{AgentError, Result};
use crate::state;

pub async fn apply(config: &AgentConfig, outbounds: &[Value], api: &BoxApi) -> Result<()> {
    let outbounds_path = config.outbounds_path();
    if tokio::fs::try_exists(&outbounds_path).await.unwrap_or(false) {
        tokio::fs::copy(&outbounds_path, config.outbounds_previous_path()).await?;
    }
    state::write_atomic(&outbounds_path, &serde_json::to_vec_pretty(outbounds)?).await?;

    let bytes = render_bytes(config, outbounds).await?;
    state::write_atomic(&config.config_path, &bytes).await?;

    if let Err(error) = restart(config).await {
        if let Err(rollback_error) = rollback(config, api).await {
            tracing::warn!(%rollback_error, "rollback failed");
        }
        return Err(error);
    }

    match wait_healthy(config, api).await {
        Ok(()) => Ok(()),
        Err(error) => {
            if let Err(rollback_error) = rollback(config, api).await {
                tracing::warn!(%rollback_error, "rollback failed");
            }
            Err(error)
        }
    }
}

async fn restart(config: &AgentConfig) -> Result<()> {
    let Some(command) = config.restart_cmd.as_deref().filter(|command| !command.is_empty()) else {
        return Ok(());
    };

    let output = tokio::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(command)
        .output()
        .await?;

    if !output.status.success() {
        return Err(AgentError::Restart(format!(
            "restart command failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

async fn wait_healthy(config: &AgentConfig, api: &BoxApi) -> Result<()> {
    let deadline = Instant::now() + config.health_timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(AgentError::Unhealthy);
        }
        let attempt = remaining.min(Duration::from_secs(1));
        if api.version_within(attempt).await.is_ok() {
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
        return Err(AgentError::Rollback(
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
    let bytes = render_bytes(config, &outbounds).await?;
    state::write_atomic(&config.config_path, &bytes).await?;
    restart(config).await?;
    wait_healthy(config, api).await
}

pub async fn render_bytes(config: &AgentConfig, outbounds: &[Value]) -> Result<Vec<u8>> {
    let template_bytes = tokio::fs::read(&config.template_path).await?;
    let template: Value = serde_json::from_slice(&template_bytes)?;
    let mut rendered = render(&template, outbounds, &config.latency_url)?;
    inject_test(&mut rendered, config, outbounds)?;
    Ok(to_bytes(&rendered))
}

/// The bandwidth probe dials its own inbound with the test selector selected,
/// so it never touches the real egress selector.
fn inject_test(config: &mut Value, agent: &AgentConfig, outbounds: &[Value]) -> Result<()> {
    let already = config
        .get("outbounds")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .any(|outbound| outbound_tag(outbound).as_deref() == Some(TEST_SELECTOR))
        })
        .unwrap_or(false);
    if already {
        return Ok(());
    }

    let tags: Vec<Value> = outbounds
        .iter()
        .filter(|outbound| is_proxy_outbound(outbound))
        .filter_map(|outbound| outbound_tag(outbound).map(Value::from))
        .collect();
    if tags.is_empty() {
        return Ok(());
    }
    let default = tags.first().cloned().unwrap_or(Value::Null);

    let root = config
        .as_object_mut()
        .ok_or_else(|| AgentError::Template("rendered config is not an object".into()))?;

    let inbounds = root.entry("inbounds").or_insert_with(|| json!([]));
    if let Some(array) = inbounds.as_array_mut() {
        array.push(json!({
            "type": "mixed",
            "tag": TEST_INBOUND_TAG,
            "listen": agent.tester.inbound.ip().to_string(),
            "listen_port": agent.tester.inbound.port(),
        }));
    }

    let list = root.entry("outbounds").or_insert_with(|| json!([]));
    if let Some(array) = list.as_array_mut() {
        array.push(json!({
            "type": "selector",
            "tag": TEST_SELECTOR,
            "outbounds": tags,
            "default": default,
        }));
    }

    let route = root.entry("route").or_insert_with(|| json!({ "rules": [] }));
    if let Some(route) = route.as_object_mut() {
        let rules = route.entry("rules").or_insert_with(|| json!([]));
        if let Some(array) = rules.as_array_mut() {
            array.insert(
                0,
                json!({ "inbound": [TEST_INBOUND_TAG], "outbound": TEST_SELECTOR }),
            );
        }
    }

    Ok(())
}

pub async fn apply_selection(api: &BoxApi, desired: &str, tags: &[String]) -> Result<String> {
    let valid = |candidate: &str| {
        matches!(candidate, AUTO_TAG | DIRECT_TAG) || tags.iter().any(|tag| tag == candidate)
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
