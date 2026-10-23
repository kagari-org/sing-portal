use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
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
        let before = self.api.outbounds().await?;
        if !before.contains_key(tag) {
            return Err(AgentError::Measure(format!(
                "outbound `{tag}` is not known to sing-box"
            )));
        }

        let triggered_at = unix_seconds();
        self.api.url_test(tag).await?;

        // urlTestTime only has second resolution, so wait past the trigger
        // second before trusting the reported delay
        tokio::time::sleep(Duration::from_millis(1200)).await;

        let deadline = Instant::now() + self.config.timeout.max(Duration::from_secs(3));
        loop {
            let after = self.api.outbounds().await?;
            if let Some(state) = after.get(tag)
                && state.url_test_time >= triggered_at
            {
                return match state.delay_ms {
                    Some(millis) => Ok(millis as f64),
                    None => Err(AgentError::Measure(format!(
                        "url test failed for outbound `{tag}`"
                    ))),
                };
            }
            if Instant::now() >= deadline {
                return match after.get(tag).and_then(|state| state.delay_ms) {
                    Some(millis) => Ok(millis as f64),
                    None => Err(AgentError::Measure(format!(
                        "url test for outbound `{tag}` did not finish in time"
                    ))),
                };
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    async fn quality(&self, tag: &str) -> Result<NetworkQuality> {
        let progress = self
            .api
            .network_quality(
                tag,
                self.config.network_quality_runtime,
                self.config.network_quality_config_url.as_deref(),
            )
            .await?;

        let report = NetworkQuality {
            idle_latency_ms: (progress.idle_latency_ms > 0).then(|| progress.idle_latency_ms as f64),
            download_mbps: (progress.download_capacity > 0)
                .then(|| progress.download_capacity as f64 / 1_000_000.0),
            upload_mbps: (progress.upload_capacity > 0)
                .then(|| progress.upload_capacity as f64 / 1_000_000.0),
            download_rpm: (progress.download_rpm > 0).then(|| progress.download_rpm as f64),
            upload_rpm: (progress.upload_rpm > 0).then(|| progress.upload_rpm as f64),
        };

        if report.download_mbps.is_none() && report.upload_mbps.is_none() {
            return Err(AgentError::Measure(
                "network quality test reported no capacity".into(),
            ));
        }
        Ok(report)
    }
}

fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}
