use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use sing_portal_core::render::TEST_SELECTOR;
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
    pub inbound: SocketAddr,
    pub download_url: String,
    pub max_bytes: u64,
}

impl Default for TesterConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(20),
            inbound: SocketAddr::from(([127, 0, 0, 1], 7899)),
            download_url: "https://speed.cloudflare.com/__down?bytes=26214400".to_string(),
            max_bytes: 5 * 1024 * 1024,
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

    fn bandwidth(tag: &str, down_mbps: f64) -> Self {
        Self {
            tag: tag.to_string(),
            kind: Kind::Bandwidth.as_str().to_string(),
            latency_ms: None,
            down_mbps: Some(down_mbps),
            up_mbps: None,
            responsiveness: None,
        }
    }

    fn both(tag: &str, latency_ms: f64, down_mbps: f64) -> Self {
        Self {
            tag: tag.to_string(),
            kind: Kind::Both.as_str().to_string(),
            latency_ms: Some(latency_ms),
            down_mbps: Some(down_mbps),
            up_mbps: None,
            responsiveness: None,
        }
    }
}

#[derive(Clone)]
pub struct Tester {
    config: TesterConfig,
    api: BoxApi,
    // the download test uses one inbound and one selector, keep it serialised
    download_lock: Arc<Mutex<()>>,
}

impl Tester {
    pub fn new(config: TesterConfig, api: BoxApi) -> Self {
        Self {
            config,
            api,
            download_lock: Arc::new(Mutex::new(())),
        }
    }

    pub async fn measure(&self, tag: &str, kind: Kind) -> Result<Measurement> {
        match kind {
            // latency probes only trigger url tests, they can run concurrently
            Kind::Latency => Ok(Measurement::latency(tag, self.latency(tag).await?)),
            Kind::Bandwidth => Ok(Measurement::bandwidth(tag, self.download(tag).await?)),
            Kind::Both => {
                let latency = self.latency(tag).await?;
                let down = self.download(tag).await?;
                Ok(Measurement::both(tag, latency, down))
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

    /// Bounded single-stream download through the dedicated test selector.
    /// When the deadline hits, whatever arrived so far is used for the rate,
    /// so slow nodes still produce a (low) number instead of a failure.
    async fn download(&self, tag: &str) -> Result<f64> {
        let _guard = self.download_lock.lock().await;
        self.api.select(TEST_SELECTOR, tag).await?;

        let proxy = format!("http://{}", self.config.inbound);
        let proxy = reqwest::Proxy::all(&proxy)
            .map_err(|error| AgentError::Measure(format!("invalid test inbound: {error}")))?;
        let client = reqwest::Client::builder()
            .proxy(proxy)
            .build()
            .map_err(|error| AgentError::Measure(format!("cannot build downloader: {error}")))?;

        let started = Instant::now();
        let deadline = started + self.config.timeout;
        let mut response = client
            .get(&self.config.download_url)
            .send()
            .await
            .map_err(|error| AgentError::Measure(format!("download failed: {error}")))?;
        if !response.status().is_success() {
            return Err(AgentError::Measure(format!(
                "download failed: http {}",
                response.status()
            )));
        }

        let mut first_byte: Option<Instant> = None;
        let mut bytes = 0u64;
        loop {
            if bytes >= self.config.max_bytes {
                break;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, response.chunk()).await {
                Ok(Ok(Some(chunk))) => {
                    first_byte.get_or_insert_with(Instant::now);
                    bytes += chunk.len() as u64;
                }
                Ok(Ok(None)) => break,
                Ok(Err(error)) => {
                    if bytes == 0 {
                        return Err(AgentError::Measure(format!("download failed: {error}")));
                    }
                    break;
                }
                Err(_) => break,
            }
        }

        if bytes == 0 {
            return Err(AgentError::Measure("download returned no data".into()));
        }

        let seconds = first_byte
            .map(|start| start.elapsed().as_secs_f64())
            .unwrap_or_else(|| started.elapsed().as_secs_f64())
            .max(0.05);
        Ok(bytes as f64 * 8.0 / seconds / 1_000_000.0)
    }
}

fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}
