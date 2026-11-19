use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sing_portal_core::model::{is_proxy_outbound, outbound_tag};
use sing_portal_core::render::{
    AUTO_TAG, DIRECT_TAG, EGRESS_INBOUND_TAG, HEALTH_INBOUND_TAG, PROBE_DNS_TAG, SELECTOR_TAG,
    TEST_INBOUND_TAG, TEST_SELECTOR, render,
};

use crate::api::BoxApi;
use crate::config::AgentConfig;
use crate::error::{AgentError, Result};
use crate::host::Host;
use crate::tester::TesterConfig;

pub fn render_text(config: &AgentConfig, outbounds: &[Value]) -> Result<String> {
    let template: Value = serde_json::from_str(&config.template)?;
    let mut rendered = render(&template, outbounds, &config.latency_url)?;
    inject_test(&mut rendered, &config.tester, outbounds)?;
    let mut text = serde_json::to_string_pretty(&rendered)?;
    text.push('\n');
    Ok(text)
}

pub fn render_bytes(config: &AgentConfig, outbounds: &[Value]) -> Result<Vec<u8>> {
    Ok(render_text(config, outbounds)?.into_bytes())
}

pub fn template_fingerprint(config: &AgentConfig) -> String {
    let mut hasher = Sha256::new();
    hasher.update(config.template.as_bytes());
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

/// Writes a config, restarts sing-box and waits for the api to answer. Returns
/// the text that came up healthy so the caller can keep it for the next
/// rollback.
pub async fn apply(
    host: &Arc<dyn Host>,
    config: &AgentConfig,
    outbounds: &[Value],
    api: &BoxApi,
    previous: Option<&str>,
) -> Result<String> {
    let rendered = render_text(config, outbounds)?;

    if let Err(error) = install(host, config, api, &rendered).await {
        if let Some(previous) = previous
            && let Err(rollback_error) = install(host, config, api, previous).await
        {
            tracing::warn!(%rollback_error, "rollback failed");
        }
        return Err(error);
    }

    Ok(rendered)
}

async fn install(
    host: &Arc<dyn Host>,
    config: &AgentConfig,
    api: &BoxApi,
    text: &str,
) -> Result<()> {
    let writer = host.clone();
    let bytes = text.as_bytes().to_vec();
    tokio::task::spawn_blocking(move || writer.write_config(&bytes))
        .await
        .map_err(|error| AgentError::Host(error.to_string()))?
        .map_err(|error| AgentError::Host(error.to_string()))?;

    let restarter = host.clone();
    tokio::task::spawn_blocking(move || restarter.restart())
        .await
        .map_err(|error| AgentError::Host(error.to_string()))?
        .map_err(|error| AgentError::Host(error.to_string()))?;

    wait_healthy(config, api).await
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
            // the api answers before the reloaded config has registered its
            // groups; wait for the selector so the caller can select on it
            let check = deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_secs(1));
            if check.is_zero() {
                return Err(AgentError::Unhealthy);
            }
            if let Ok(Ok(_)) = tokio::time::timeout(check, api.current_selection()).await {
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            return Err(AgentError::Unhealthy);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// The bandwidth probe dials its own inbound with the test selector selected,
/// so it never touches the real egress selector. The egress probe uses a
/// second inbound routed to `s_test`, with a dedicated DNS server detouring
/// through the same selector. The portal can therefore verify a candidate
/// (including DNS) while `s_out` stays untouched, and switch only once.
fn inject_test(config: &mut Value, tester: &TesterConfig, outbounds: &[Value]) -> Result<()> {
    let tags: Vec<Value> = outbounds
        .iter()
        .filter(|outbound| is_proxy_outbound(outbound))
        .filter_map(|outbound| outbound_tag(outbound).map(Value::from))
        .collect();
    if tags.is_empty() {
        return Ok(());
    }

    let resolver = config
        .get("route")
        .and_then(|route| route.get("default_domain_resolver"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let dns_final = config
        .get("dns")
        .and_then(|dns| dns.get("final"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let final_server = dns_final.as_deref().and_then(|final_tag| {
        config
            .get("dns")
            .and_then(|dns| dns.get("servers"))
            .and_then(Value::as_array)
            .and_then(|servers| {
                servers
                    .iter()
                    .find(|server| server.get("tag").and_then(Value::as_str) == Some(final_tag))
            })
    });
    let has_real_final = final_server
        .is_some_and(|server| server.get("type").and_then(Value::as_str) != Some("fakeip"));
    // Mirror the real final resolver only when it detours through an outbound.
    // If the template resolves directly, DNS does not depend on the candidate
    // and the probe should not demand DNS through it.
    let probe_dns_source = final_server
        .filter(|server| {
            server.get("detour").is_some()
                && server.get("type").and_then(Value::as_str) != Some("fakeip")
        })
        .cloned();

    let root = config
        .as_object_mut()
        .ok_or_else(|| AgentError::Template("rendered config is not an object".into()))?;

    let inbounds = root.entry("inbounds").or_insert_with(|| json!([]));
    if let Some(array) = inbounds.as_array_mut() {
        let present = |array: &Vec<Value>, tag: &str| {
            array
                .iter()
                .any(|inbound| inbound.get("tag").and_then(Value::as_str) == Some(tag))
        };
        if !present(array, TEST_INBOUND_TAG) {
            array.push(json!({
                "type": "mixed",
                "tag": TEST_INBOUND_TAG,
                "listen": tester.inbound.ip().to_string(),
                "listen_port": tester.inbound.port(),
            }));
        }
        if !present(array, EGRESS_INBOUND_TAG) {
            array.push(json!({
                "type": "mixed",
                "tag": EGRESS_INBOUND_TAG,
                "listen": tester.egress_inbound.ip().to_string(),
                "listen_port": tester.egress_inbound.port(),
            }));
        }
        if !present(array, HEALTH_INBOUND_TAG) {
            array.push(json!({
                "type": "mixed",
                "tag": HEALTH_INBOUND_TAG,
                "listen": tester.health_inbound.ip().to_string(),
                "listen_port": tester.health_inbound.port(),
            }));
        }
    }

    let list = root.entry("outbounds").or_insert_with(|| json!([]));
    if let Some(array) = list.as_array_mut() {
        let present = array
            .iter()
            .any(|outbound| outbound_tag(outbound) == Some(TEST_SELECTOR));
        if !present {
            let default = tags.first().cloned().unwrap_or(Value::Null);
            array.push(json!({
                "type": "selector",
                "tag": TEST_SELECTOR,
                "outbounds": tags,
                "default": default,
            }));
        }
    }

    let mut probe_dns_ready = false;
    if let Some(mut probe) = probe_dns_source
        && let Some(dns) = root.get_mut("dns").and_then(Value::as_object_mut)
    {
        if let Some(object) = probe.as_object_mut() {
            object.insert("tag".into(), Value::String(PROBE_DNS_TAG.into()));
            object.insert("detour".into(), Value::String(TEST_SELECTOR.into()));
        }

        let servers = dns.entry("servers").or_insert_with(|| json!([]));
        if let Some(array) = servers.as_array_mut() {
            let present = array
                .iter()
                .any(|server| server.get("tag").and_then(Value::as_str) == Some(PROBE_DNS_TAG));
            if !present {
                // Same resolver as real browsing, but detoured through s_test.
                array.push(probe);
            }
            probe_dns_ready = true;
        }
    }

    let route = root
        .entry("route")
        .or_insert_with(|| json!({ "rules": [] }));
    if let Some(route) = route.as_object_mut() {
        let rules = route.entry("rules").or_insert_with(|| json!([]));
        if let Some(array) = rules.as_array_mut() {
            let has_outbound_rule = |inbound: &str| {
                array.iter().any(|rule| {
                    rule.get("outbound").is_some()
                        && rule
                            .get("inbound")
                            .and_then(Value::as_array)
                            .map(|list| list.iter().any(|value| value.as_str() == Some(inbound)))
                            .unwrap_or(false)
                })
            };
            let has_resolve_rule = |inbound: &str| {
                array.iter().any(|rule| {
                    rule.get("action").and_then(Value::as_str) == Some("resolve")
                        && rule
                            .get("inbound")
                            .and_then(Value::as_array)
                            .map(|list| list.iter().any(|value| value.as_str() == Some(inbound)))
                            .unwrap_or(false)
                })
            };

            let has_egress_rule = has_outbound_rule(EGRESS_INBOUND_TAG);
            let has_test_rule = has_outbound_rule(TEST_INBOUND_TAG);
            let has_health_rule = has_outbound_rule(HEALTH_INBOUND_TAG);
            let has_egress_resolve = has_resolve_rule(EGRESS_INBOUND_TAG);
            let has_test_resolve = has_resolve_rule(TEST_INBOUND_TAG);
            let has_health_resolve = has_resolve_rule(HEALTH_INBOUND_TAG);

            if !has_health_rule {
                array.insert(
                    0,
                    json!({ "inbound": [HEALTH_INBOUND_TAG], "outbound": SELECTOR_TAG }),
                );
            }
            if !has_egress_rule {
                array.insert(
                    0,
                    json!({ "inbound": [EGRESS_INBOUND_TAG], "outbound": TEST_SELECTOR }),
                );
            }
            if !has_test_rule {
                array.insert(
                    0,
                    json!({ "inbound": [TEST_INBOUND_TAG], "outbound": TEST_SELECTOR }),
                );
            }
            if has_real_final && !has_health_resolve {
                // The watchdog must exercise the same DNS path as browsing,
                // including evaluate/race rules, through the current s_out.
                array.insert(
                    0,
                    json!({
                        "inbound": [HEALTH_INBOUND_TAG],
                        "action": "resolve",
                    }),
                );
            }
            if probe_dns_ready && !has_egress_resolve {
                array.insert(
                    0,
                    json!({
                        "inbound": [EGRESS_INBOUND_TAG],
                        "action": "resolve",
                        "server": PROBE_DNS_TAG,
                    }),
                );
            }
            if let Some(resolver) = resolver
                && !has_test_resolve
            {
                // Resolve the download host independently from the current
                // egress; the egress check is what validates DNS via s_test.
                array.insert(
                    0,
                    json!({
                        "inbound": [TEST_INBOUND_TAG],
                        "action": "resolve",
                        "server": resolver,
                    }),
                );
            }
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
