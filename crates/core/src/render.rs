use serde_json::{Map, Value, json};

use crate::error::Error;
use crate::model::{is_proxy_outbound, outbound_tag};

pub const SELECTOR_TAG: &str = "s_out";
pub const AUTO_TAG: &str = "s_auto";
pub const DIRECT_TAG: &str = "s_direct";
pub const TEST_SELECTOR: &str = "s_test";
pub const TEST_INBOUND_TAG: &str = "s_test-in";
pub const EGRESS_INBOUND_TAG: &str = "s_egress-in";
pub const HEALTH_INBOUND_TAG: &str = "s_health-in";
pub const PROBE_DNS_TAG: &str = "s_probe-dns";

pub fn render(template: &Value, outbounds: &[Value], test_url: &str) -> Result<Value, Error> {
    let mut config = template
        .as_object()
        .cloned()
        .ok_or(Error::TemplateNotObject)?;

    if config.contains_key("outbounds") {
        return Err(Error::TemplateHasOutbounds);
    }

    let defaults = match config.remove("outbound_defaults") {
        Some(Value::Object(defaults)) => Some(defaults),
        Some(_) => return Err(Error::TemplateOutboundDefaults),
        None => None,
    };

    let nodes: Vec<Value> = outbounds
        .iter()
        .filter(|outbound| is_proxy_outbound(outbound))
        .map(|outbound| with_defaults(outbound, defaults.as_ref()))
        .collect();
    let tags: Vec<Value> = nodes
        .iter()
        .filter_map(|outbound| outbound_tag(outbound).map(Value::from))
        .collect();

    let mut rendered = Vec::with_capacity(nodes.len() + 3);
    if tags.is_empty() {
        rendered.push(json!({
            "type": "selector",
            "tag": SELECTOR_TAG,
            "outbounds": [DIRECT_TAG],
            "default": DIRECT_TAG,
        }));
    } else {
        let mut choices = Vec::with_capacity(tags.len() + 2);
        choices.push(Value::from(AUTO_TAG));
        choices.push(Value::from(DIRECT_TAG));
        choices.extend(tags.iter().cloned());
        rendered.push(json!({
            "type": "selector",
            "tag": SELECTOR_TAG,
            "outbounds": choices,
            "default": AUTO_TAG,
        }));
        rendered.push(json!({
            "type": "urltest",
            "tag": AUTO_TAG,
            "outbounds": tags,
            "url": test_url,
            "interval": "5m",
            "tolerance": 50,
        }));
    }
    rendered.push(json!({ "type": "direct", "tag": DIRECT_TAG }));
    rendered.extend(nodes);

    config.insert("outbounds".to_string(), Value::Array(rendered));
    Ok(Value::Object(config))
}

/// Template-level dial defaults. Per-outbound values always win, so a
/// subscription that already pins a field keeps its own setting.
fn with_defaults(outbound: &Value, defaults: Option<&Map<String, Value>>) -> Value {
    let mut outbound = outbound.clone();
    let Some(defaults) = defaults else {
        return outbound;
    };
    let Some(object) = outbound.as_object_mut() else {
        return outbound;
    };
    for (key, value) in defaults {
        object.entry(key.clone()).or_insert_with(|| value.clone());
    }
    outbound
}

pub fn to_bytes(config: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(config).unwrap_or_default();
    bytes.push(b'\n');
    bytes
}
