use serde_json::{Value, json};

use crate::error::Error;
use crate::model::{is_proxy_outbound, outbound_tag};

pub const SELECTOR_TAG: &str = "s_out";
pub const AUTO_TAG: &str = "s_auto";
pub const DIRECT_TAG: &str = "s_direct";

pub fn render(template: &Value, outbounds: &[Value], test_url: &str) -> Result<Value, Error> {
    let mut config = template
        .as_object()
        .cloned()
        .ok_or(Error::TemplateNotObject)?;

    if config.contains_key("outbounds") {
        return Err(Error::TemplateHasOutbounds);
    }

    let nodes: Vec<Value> = outbounds
        .iter()
        .filter(|outbound| is_proxy_outbound(outbound))
        .cloned()
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

pub fn to_bytes(config: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(config).unwrap_or_default();
    bytes.push(b'\n');
    bytes
}
