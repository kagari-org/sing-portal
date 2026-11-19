use std::time::Duration;

use crate::tester::TesterConfig;

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub portal_url: String,
    pub node_name: String,
    pub token: Option<String>,
    /// The sing-box template as JSON text; the host owns where it lives.
    pub template: String,
    pub api_url: String,
    pub latency_url: String,
    pub interval: Duration,
    pub once: bool,
    pub best_effort: bool,
    pub health_timeout: Duration,
    pub egress_health_interval: Duration,
    pub egress_health_failures: u32,
    pub tester: TesterConfig,
}
