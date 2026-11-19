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
    /// libbox's CommandServer secret, sent as `x-command-secret` metadata.
    pub api_secret: Option<String>,
    pub latency_url: String,
    pub interval: Duration,
    pub once: bool,
    pub best_effort: bool,
    /// Serve automatic evaluation probes too. A phone turns this off so its
    /// battery and data are only spent on tests someone asked for.
    pub auto_measure: bool,
    pub health_timeout: Duration,
    pub egress_health_interval: Duration,
    pub egress_health_failures: u32,
    pub tester: TesterConfig,
}
