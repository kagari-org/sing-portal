use serde::Serialize;
use sing_portal_store::{NodeReport, Subscription};

use crate::config::mask_url;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionView {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub source: String,
    pub url: Option<String>,
    pub user_agent: String,
    pub format: String,
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
    pub outbound_count: usize,
    pub outbound_tags: Vec<String>,
}

impl From<&Subscription> for SubscriptionView {
    fn from(subscription: &Subscription) -> Self {
        Self {
            id: subscription.id.clone(),
            name: subscription.name.clone(),
            slug: subscription.slug.clone(),
            source: subscription.source.as_str().to_string(),
            url: subscription.url.as_deref().map(mask_url),
            user_agent: subscription.user_agent.clone(),
            format: subscription.format.clone(),
            enabled: subscription.enabled,
            auto_refresh: subscription.auto_refresh,
            refresh_interval_secs: subscription.refresh_interval_secs,
            next_refresh_at: subscription.next_refresh_at,
            last_fetched_at: subscription.last_fetched_at,
            last_error: subscription.last_error.clone(),
            validated: subscription.validated,
            failures: subscription.failures,
            created_at: subscription.created_at,
            updated_at: subscription.updated_at,
            outbound_count: subscription.outbounds.len(),
            outbound_tags: subscription.outbound_tags(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MutationView {
    pub subscription: SubscriptionView,
    pub warnings: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportView {
    pub revision: String,
    pub selection: String,
    pub health: String,
    pub error: String,
    pub reported_at: i64,
    pub online: bool,
}

impl ReportView {
    pub fn new(report: &NodeReport, online: bool) -> Self {
        Self {
            revision: report.revision.clone(),
            selection: report.selection.clone(),
            health: report.health.clone(),
            error: report.error.clone(),
            reported_at: report.reported_at,
            online,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeView {
    pub name: String,
    pub address: String,
    pub selection: String,
    pub mode: String,
    pub score: Option<f64>,
    pub best: Option<String>,
    pub best_score: Option<f64>,
    pub measured_at: Option<i64>,
    pub reachable: bool,
    pub version: Option<String>,
    pub report: Option<ReportView>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeasurementView {
    pub tag: String,
    pub latency_ms: Option<f64>,
    pub down_mbps: Option<f64>,
    pub score: f64,
    pub measured_at: i64,
    pub best: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionResponse {
    pub selection: String,
    pub applied: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeStateView {
    pub revision: String,
    pub selection_revision: String,
    pub selection: String,
    pub outbounds: Vec<serde_json::Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevisionView {
    pub revision: String,
    pub outbounds: usize,
    pub enabled_subscriptions: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderNodeView {
    pub tag: String,
    pub kind: String,
    pub server: String,
    pub server_port: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderView {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub format: String,
    pub enabled: bool,
    pub last_error: String,
    pub last_fetched_at: Option<i64>,
    pub nodes: Vec<ProviderNodeView>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestResponse {
    pub tag: String,
    pub kind: String,
    pub latency_ms: Option<f64>,
    pub down_mbps: Option<f64>,
    pub up_mbps: Option<f64>,
    pub responsiveness: Option<f64>,
    pub source: String,
}
