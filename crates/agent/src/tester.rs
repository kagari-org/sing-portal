use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Value, json};
use sing_portal_core::clash_api::ClashClient;
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;

use crate::error::{AgentError, Result};
use crate::state;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Latency,
    Bandwidth,
    Throughput,
    Both,
}

impl Kind {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "latency" => Some(Kind::Latency),
            "bandwidth" => Some(Kind::Bandwidth),
            "throughput" => Some(Kind::Throughput),
            "both" => Some(Kind::Both),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Latency => "latency",
            Kind::Bandwidth => "bandwidth",
            Kind::Throughput => "throughput",
            Kind::Both => "both",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TesterConfig {
    pub sing_box: PathBuf,
    pub state_dir: PathBuf,
    pub latency_url: String,
    pub download_url: String,
    pub max_bytes: u64,
    pub timeout: Duration,
    pub network_quality_runtime: u64,
}

impl Default for TesterConfig {
    fn default() -> Self {
        Self {
            sing_box: PathBuf::from("sing-box"),
            state_dir: PathBuf::from("/var/lib/sing-box"),
            latency_url: "https://www.gstatic.com/generate_204".to_string(),
            download_url: "https://speed.cloudflare.com/__down?bytes=10485760".to_string(),
            max_bytes: 10 * 1024 * 1024,
            timeout: Duration::from_secs(20),
            network_quality_runtime: 8,
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
    pub bytes: Option<u64>,
    pub seconds: Option<f64>,
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
            bytes: None,
            seconds: None,
        }
    }

    fn throughput(tag: &str, bytes: u64, seconds: f64, latency_ms: Option<f64>) -> Self {
        Self {
            tag: tag.to_string(),
            kind: if latency_ms.is_some() {
                Kind::Both.as_str().to_string()
            } else {
                Kind::Throughput.as_str().to_string()
            },
            latency_ms,
            down_mbps: Some(mbps(bytes, seconds)),
            up_mbps: None,
            responsiveness: None,
            bytes: Some(bytes),
            seconds: Some(seconds),
        }
    }

    fn network_quality(tag: &str, report: NetworkQuality) -> Self {
        Self {
            tag: tag.to_string(),
            kind: Kind::Bandwidth.as_str().to_string(),
            latency_ms: report.idle_latency_ms,
            down_mbps: report.download_mbps,
            up_mbps: report.upload_mbps,
            responsiveness: report.download_rpm,
            bytes: None,
            seconds: None,
        }
    }
}

pub fn mbps(bytes: u64, seconds: f64) -> f64 {
    if seconds <= 0.0 {
        return 0.0;
    }
    bytes as f64 * 8.0 / seconds / 1_000_000.0
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
                let report = self.network_quality(tag).await?;
                Ok(Measurement::network_quality(tag, report))
            }
            Kind::Throughput => {
                let (bytes, seconds) = self
                    .throughput(tag, url.unwrap_or(&self.config.download_url))
                    .await?;
                Ok(Measurement::throughput(tag, bytes, seconds, None))
            }
            Kind::Both => {
                let latency = self
                    .latency(tag, url.unwrap_or(&self.config.latency_url))
                    .await?;
                let (bytes, seconds) = self
                    .throughput(tag, url.unwrap_or(&self.config.download_url))
                    .await?;
                Ok(Measurement::throughput(tag, bytes, seconds, Some(latency)))
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

    async fn network_quality(&self, tag: &str) -> Result<NetworkQuality> {
        let probe_config = self.write_probe_config(tag).await?;
        let runtime = self.config.network_quality_runtime.to_string();

        let output = tokio::time::timeout(
            Duration::from_secs(self.config.network_quality_runtime + 30),
            tokio::process::Command::new(&self.config.sing_box)
                .args(["tools", "networkquality"])
                .args(["-o", tag])
                .args(["-c", &probe_config.to_string_lossy()])
                .args(["--max-runtime", &runtime])
                .output(),
        )
        .await
        .map_err(|_| AgentError::Measure("network quality test timed out".into()))?
        .map_err(|error| {
            AgentError::Measure(format!("cannot run network quality test: {error}"))
        })?;

        let _ = tokio::fs::remove_file(&probe_config).await;

        if !output.status.success() {
            return Err(AgentError::Measure(format!(
                "network quality test failed: {}",
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

    async fn throughput(&self, tag: &str, url: &str) -> Result<(u64, f64)> {
        let probe_config = self.write_probe_config(tag).await?;

        let mut child = tokio::process::Command::new(&self.config.sing_box)
            .args(["tools", "fetch"])
            .args(["-o", tag])
            .args(["-c", &probe_config.to_string_lossy()])
            .arg(url)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| AgentError::Measure(format!("cannot run fetch: {error}")))?;

        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| AgentError::Measure("fetch has no stdout".into()))?;

        let started = Instant::now();
        let deadline = started + self.config.timeout;
        let mut first_byte: Option<Instant> = None;
        let mut bytes = 0u64;
        let mut buffer = vec![0u8; 64 * 1024];
        let mut failure = None;

        loop {
            if bytes >= self.config.max_bytes || Instant::now() >= deadline {
                break;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(remaining, stdout.read(&mut buffer)).await {
                Ok(Ok(0)) => break,
                Ok(Ok(read)) => {
                    first_byte.get_or_insert_with(Instant::now);
                    bytes += read as u64;
                }
                Ok(Err(error)) => {
                    failure = Some(format!("fetch failed: {error}"));
                    break;
                }
                Err(_) => break,
            }
        }

        let _ = child.kill().await;
        let status = child.wait().await.ok();
        let _ = tokio::fs::remove_file(&probe_config).await;

        if let Some(failure) = failure {
            return Err(AgentError::Measure(failure));
        }
        if bytes == 0 {
            return Err(AgentError::Measure(format!(
                "fetch returned no data (exit: {status:?})"
            )));
        }

        let seconds = first_byte
            .map(|start| start.elapsed().as_secs_f64())
            .unwrap_or_else(|| started.elapsed().as_secs_f64())
            .max(0.001);

        Ok((bytes, seconds))
    }

    async fn write_probe_config(&self, tag: &str) -> Result<PathBuf> {
        let outbounds_path = self.config.state_dir.join("outbounds.json");
        let bytes = tokio::fs::read(&outbounds_path)
            .await
            .map_err(|error| AgentError::Measure(format!("cannot read cached outbounds: {error}")))?;
        let outbounds: Vec<Value> = serde_json::from_slice(&bytes)?;
        let outbound = outbounds
            .iter()
            .find(|outbound| outbound.get("tag").and_then(Value::as_str) == Some(tag))
            .ok_or_else(|| AgentError::Measure(format!("outbound `{tag}` is not in the cache")))?;

        let config = json!({
            "log": { "level": "error" },
            "outbounds": [outbound],
        });
        let path = self.config.state_dir.join("probe.json");
        let bytes = serde_json::to_vec(&config)?;
        state::write_atomic(&path, &bytes).await?;
        Ok(path)
    }

    pub fn binary(&self) -> &Path {
        &self.config.sing_box
    }
}
