use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;
use sing_portal_core::clash_api::ClashClient;
use tokio::sync::Mutex;

use crate::error::{AgentError, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Latency,
    Bandwidth,
    Both,
}

impl Kind {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "latency" => Some(Kind::Latency),
            "bandwidth" => Some(Kind::Bandwidth),
            "both" => Some(Kind::Both),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Latency => "latency",
            Kind::Bandwidth => "bandwidth",
            Kind::Both => "both",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TesterConfig {
    pub sing_box: PathBuf,
    pub api_url: Option<String>,
    pub api_secret: Option<String>,
    pub latency_url: String,
    pub timeout: Duration,
    pub network_quality_runtime: u64,
    pub network_quality_config_url: Option<String>,
}

impl Default for TesterConfig {
    fn default() -> Self {
        Self {
            sing_box: PathBuf::from("sing-box"),
            api_url: None,
            api_secret: None,
            latency_url: "https://www.gstatic.com/generate_204".to_string(),
            timeout: Duration::from_secs(20),
            network_quality_runtime: 8,
            network_quality_config_url: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Measurement {
    pub tag: String,
    pub kind: String,
    pub latency_ms: Option<f64>,
    pub down_mbps: Option<f64>,
    pub up_mbps: Option<f64>,
    pub responsiveness: Option<f64>,
}

impl Measurement {
    fn latency(tag: &str, latency_ms: f64) -> Self {
        Self {
            tag: tag.to_string(),
            kind: Kind::Latency.as_str().to_string(),
            latency_ms: Some(latency_ms),
            down_mbps: None,
            up_mbps: None,
            responsiveness: None,
        }
    }

    fn bandwidth(tag: &str, report: &NetworkQuality) -> Self {
        Self {
            tag: tag.to_string(),
            kind: Kind::Bandwidth.as_str().to_string(),
            latency_ms: report.idle_latency_ms,
            down_mbps: report.download_mbps,
            up_mbps: report.upload_mbps,
            responsiveness: report.download_rpm,
        }
    }

    fn both(tag: &str, latency_ms: f64, report: &NetworkQuality) -> Self {
        Self {
            tag: tag.to_string(),
            kind: Kind::Both.as_str().to_string(),
            latency_ms: Some(latency_ms),
            down_mbps: report.download_mbps,
            up_mbps: report.upload_mbps,
            responsiveness: report.download_rpm,
        }
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct NetworkQuality {
    pub idle_latency_ms: Option<f64>,
    pub download_mbps: Option<f64>,
    pub upload_mbps: Option<f64>,
    pub download_rpm: Option<f64>,
    pub upload_rpm: Option<f64>,
}

pub fn parse_network_quality(output: &str) -> NetworkQuality {
    let text = output.replace('\r', "\n");
    NetworkQuality {
        idle_latency_ms: last_number(&text, "Idle Latency:"),
        download_mbps: last_number(&text, "Download Capacity:"),
        upload_mbps: last_number(&text, "Upload Capacity:"),
        download_rpm: last_number(&text, "Download Responsiveness:"),
        upload_rpm: last_number(&text, "Upload Responsiveness:"),
    }
}

fn last_number(text: &str, prefix: &str) -> Option<f64> {
    let mut found = None;
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix(prefix) else {
            continue;
        };
        let Some(token) = rest.split_whitespace().next() else {
            continue;
        };
        if let Ok(value) = token.parse::<f64>() {
            found = Some(value);
        }
    }
    found
}

pub struct Tester {
    config: TesterConfig,
    clash: ClashClient,
    lock: Mutex<()>,
}

impl Tester {
    pub fn new(config: TesterConfig, clash: ClashClient) -> Result<Self> {
        Ok(Self {
            config,
            clash,
            lock: Mutex::new(()),
        })
    }

    pub async fn measure(&self, tag: &str, kind: Kind, url: Option<&str>) -> Result<Measurement> {
        let _guard = self.lock.lock().await;

        match kind {
            Kind::Latency => {
                let latency = self
                    .latency(tag, url.unwrap_or(&self.config.latency_url))
                    .await?;
                Ok(Measurement::latency(tag, latency))
            }
            Kind::Bandwidth => {
                let report = self.quality(tag).await?;
                Ok(Measurement::bandwidth(tag, &report))
            }
            Kind::Both => {
                let latency = self
                    .latency(tag, url.unwrap_or(&self.config.latency_url))
                    .await?;
                let report = self.quality(tag).await?;
                Ok(Measurement::both(tag, latency, &report))
            }
        }
    }

    async fn latency(&self, tag: &str, url: &str) -> Result<f64> {
        self.clash
            .delay(tag, url, self.config.timeout.as_millis() as u64)
            .await
            .map(|delay| delay as f64)
            .map_err(|error| AgentError::Measure(error.to_string()))
    }

    async fn quality(&self, tag: &str) -> Result<NetworkQuality> {
        let Some(api_url) = self.config.api_url.as_deref() else {
            return Err(AgentError::Measure(
                "sing-box api is not configured; set --api-url so the test runs inside the daemon"
                    .into(),
            ));
        };

        let runtime = self.config.network_quality_runtime.to_string();
        let mut command = tokio::process::Command::new(&self.config.sing_box);
        command
            .args(["api", "networkquality"])
            .args(["--url", api_url])
            .args(["-o", tag])
            .args(["--max-runtime", &runtime]);
        if let Some(secret) = &self.config.api_secret {
            command.args(["--secret", secret]);
        }
        if let Some(config_url) = &self.config.network_quality_config_url {
            command.args(["--config-url", config_url]);
        }

        let output = tokio::time::timeout(
            Duration::from_secs(self.config.network_quality_runtime + 30),
            command.output(),
        )
        .await
        .map_err(|_| AgentError::Measure("network quality test timed out".into()))?
        .map_err(|error| AgentError::Measure(format!("cannot call the sing-box api: {error}")))?;

        if !output.status.success() {
            return Err(AgentError::Measure(format!(
                "sing-box api networkquality failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }

        let report = parse_network_quality(&String::from_utf8_lossy(&output.stdout));
        if report.download_mbps.is_none() && report.upload_mbps.is_none() {
            return Err(AgentError::Measure(
                "network quality test reported no capacity".into(),
            ));
        }
        Ok(report)
    }
}
