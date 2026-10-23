use std::time::{Duration, Instant};

use serde::Serialize;
use sing_portal_core::render::AUTO_TAG;
use tokio::sync::Mutex;

use crate::api::BoxApi;
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
    pub timeout: Duration,
    pub network_quality_runtime: u64,
    pub network_quality_config_url: Option<String>,
}

impl Default for TesterConfig {
    fn default() -> Self {
        Self {
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
    api: BoxApi,
    lock: Mutex<()>,
}

impl Tester {
    pub fn new(config: TesterConfig, api: BoxApi) -> Self {
        Self {
            config,
            api,
            lock: Mutex::new(()),
        }
    }

    pub async fn measure(&self, tag: &str, kind: Kind) -> Result<Measurement> {
        let _guard = self.lock.lock().await;

        match kind {
            Kind::Latency => Ok(Measurement::latency(tag, self.latency(tag).await?)),
            Kind::Bandwidth => {
                let report = self.quality(tag).await?;
                Ok(Measurement::bandwidth(tag, &report))
            }
            Kind::Both => {
                let latency = self.latency(tag).await?;
                let report = self.quality(tag).await?;
                Ok(Measurement::both(tag, latency, &report))
            }
        }
    }

    async fn latency(&self, tag: &str) -> Result<f64> {
        let before = self.api.delays().await?;
        if !before.contains_key(tag) {
            return Err(AgentError::Measure(format!(
                "outbound `{tag}` is not part of the {AUTO_TAG} group"
            )));
        }

        self.api.url_test(AUTO_TAG).await?;
        let deadline = Instant::now() + self.config.timeout.max(Duration::from_secs(3));

        loop {
            let after = self.api.delays().await?;
            if after.get(tag) != before.get(tag) {
                return match after.get(tag) {
                    Some(Some(millis)) => Ok(*millis as f64),
                    _ => Err(AgentError::Measure(format!(
                        "url test failed for outbound `{tag}`"
                    ))),
                };
            }
            if Instant::now() >= deadline {
                return match after.get(tag) {
                    Some(Some(millis)) => Ok(*millis as f64),
                    _ => Err(AgentError::Measure(format!(
                        "url test for outbound `{tag}` did not finish in time"
                    ))),
                };
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    async fn quality(&self, tag: &str) -> Result<NetworkQuality> {
        let output = self
            .api
            .network_quality(
                tag,
                self.config.network_quality_runtime,
                self.config.network_quality_config_url.as_deref(),
            )
            .await?;

        let report = parse_network_quality(&output);
        if report.download_mbps.is_none() && report.upload_mbps.is_none() {
            return Err(AgentError::Measure(
                "network quality test reported no capacity".into(),
            ));
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::parse_network_quality;

    #[test]
    fn parses_network_quality_summary() {
        let output = "==== NETWORK QUALITY TEST ====\rIdle Latency: 114 ms\r\n\
                      Download Capacity: 12.5 Mbps Accuracy: Medium\r\n\
                      Download Capacity: 88.0 Mbps Accuracy: High\r\n\
                      Upload Capacity: 42.0 Mbps Accuracy: High\r\n\
                      Download Responsiveness: 600 RPM Accuracy: Medium\r\n\
                      Upload Responsiveness: 520 RPM Accuracy: Low\r\n";
        let report = parse_network_quality(output);
        assert_eq!(report.idle_latency_ms, Some(114.0));
        assert_eq!(report.download_mbps, Some(88.0));
        assert_eq!(report.upload_mbps, Some(42.0));
        assert_eq!(report.download_rpm, Some(600.0));
    }
}
