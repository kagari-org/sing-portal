use serde_json::Value;

pub const TAG_SEPARATOR: &str = " · ";
pub const MAX_OUTBOUNDS_PER_SUBSCRIPTION: usize = 1000;
pub const DEFAULT_TEST_URL: &str = "https://www.gstatic.com/generate_204";

pub fn outbound_type(outbound: &Value) -> Option<&str> {
    outbound.get("type").and_then(Value::as_str)
}

pub fn outbound_tag(outbound: &Value) -> Option<&str> {
    outbound.get("tag").and_then(Value::as_str)
}

pub fn is_proxy_outbound(outbound: &Value) -> bool {
    match outbound_type(outbound) {
        Some(kind) => !matches!(kind, "selector" | "urltest" | "direct" | "block" | "dns"),
        None => false,
    }
}
