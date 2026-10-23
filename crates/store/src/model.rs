use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Url,
    Manual,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Url => "url",
            Source::Manual => "manual",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "url" => Some(Source::Url),
            "manual" => Some(Source::Manual),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscription {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub source: Source,
    pub url: Option<String>,
    pub user_agent: String,
    pub format: String,
    pub content: String,
    pub outbounds: Vec<Value>,
    pub enabled: bool,
    pub auto_refresh: bool,
    pub refresh_interval_secs: i64,
    pub next_refresh_at: Option<i64>,
    pub last_fetched_at: Option<i64>,
    pub last_error: String,
    pub validated: bool,
    pub failures: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Subscription {
    pub fn outbound_tags(&self) -> Vec<String> {
        self.outbounds
            .iter()
            .filter_map(|outbound| outbound.get("tag").and_then(Value::as_str))
            .map(ToString::to_string)
            .collect()
    }
}

#[derive(Debug, Clone)]
pub struct NewSubscription {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub source: Source,
    pub url: Option<String>,
    pub user_agent: String,
    pub format: String,
    pub content: String,
    pub outbounds: Vec<Value>,
    pub enabled: bool,
    pub auto_refresh: bool,
    pub refresh_interval_secs: i64,
    pub next_refresh_at: Option<i64>,
    pub validated: bool,
    pub failures: i64,
    pub now: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SubscriptionPatch {
    pub name: Option<String>,
    pub url: Option<String>,
    pub user_agent: Option<String>,
    pub enabled: Option<bool>,
    pub auto_refresh: Option<bool>,
    pub refresh_interval_secs: Option<i64>,
    pub next_refresh_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Selection {
    pub node_name: String,
    pub outbound: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeReport {
    pub node_name: String,
    pub revision: String,
    pub selection: String,
    pub health: String,
    pub error: String,
    pub reported_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: i64,
    pub at: i64,
    pub actor: String,
    pub action: String,
    pub target: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: u32,
    pub exported_at: i64,
    pub subscriptions: Vec<Subscription>,
    pub selections: Vec<Selection>,
}
