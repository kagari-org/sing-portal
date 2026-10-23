use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sing_portal_core::clash_api::ClashClient;
use sing_portal_core::render::SELECTOR_TAG;
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
    pub proxy_inbound: String,
    pub latency_url: String,
    pub download_url: String,
    pub max_bytes: usize,
    pub timeout: Duration,
}

impl Default for TesterConfig {
    fn default() -> Self {
        Self {
            proxy_inbound: "http://127.0.0.1:7897".to_string(),
            latency_url: "https://www.gstatic.com/generate_204".to_string(),
            download_url: "https://speed.cloudflare.com/__down?bytes=10485760".to_string(),
            max_bytes: 10 * 1024 * 1024,
            timeout: Duration::from_secs(20),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Measurement {
    pub tag: String,
    pub kind: String,
    pub latency_ms: Option<f64>,
    pub bytes: Option<u64>,
    pub seconds: Option<f64>,
    pub down_mbps: Option<f64>,
    pub restored: String,
}

impl Measurement {
    fn latency(tag: &str, elapsed: Duration, restored: &str) -> Self {
        Self {
            tag: tag.to_string(),
            kind: Kind::Latency.as_str().to_string(),
            latency_ms: Some(elapsed.as_secs_f64() * 1000.0),
            bytes: None,
            seconds: None,
            down_mbps: None,
            restored: restored.to_string(),
        }
    }

    fn combined(
        tag: &str,
        latency: Duration,
        bandwidth: Option<(u64, f64)>,
        restored: &str,
    ) -> Self {
        let (bytes, seconds, down) = match bandwidth {
            Some((bytes, seconds)) => (
                Some(bytes),
                Some(seconds),
                Some(down_mbps(bytes, seconds)),
            ),
            None => (None, None, None),
        };
        Self {
            tag: tag.to_string(),
            kind: Kind::Both.as_str().to_string(),
            latency_ms: Some(latency.as_secs_f64() * 1000.0),
            bytes,
            seconds,
            down_mbps: down,
            restored: restored.to_string(),
        }
    }

    fn bandwidth(tag: &str, bytes: u64, seconds: f64, restored: &str) -> Self {
        Self {
            tag: tag.to_string(),
            kind: Kind::Bandwidth.as_str().to_string(),
            latency_ms: None,
            bytes: Some(bytes),
            seconds: Some(seconds),
            down_mbps: Some(down_mbps(bytes, seconds)),
            restored: restored.to_string(),
        }
    }
}

pub fn down_mbps(bytes: u64, seconds: f64) -> f64 {
    if seconds <= 0.0 {
        return 0.0;
    }
    bytes as f64 * 8.0 / seconds / 1_000_000.0
}

pub struct Tester {
    config: TesterConfig,
    clash: ClashClient,
    proxied: reqwest::Client,
    lock: Mutex<()>,
}

impl Tester {
    pub fn new(config: TesterConfig, clash: ClashClient) -> Result<Self> {
        let proxied = reqwest::Client::builder()
            .proxy(
                reqwest::Proxy::all(&config.proxy_inbound)
                    .map_err(|error| AgentError::Proxy(error.to_string()))?,
            )
            .timeout(config.timeout)
            .build()
            .map_err(|error| AgentError::Proxy(error.to_string()))?;

        Ok(Self {
            config,
            clash,
            proxied,
            lock: Mutex::new(()),
        })
    }

    pub async fn measure(&self, tag: &str, kind: Kind, url: Option<&str>) -> Result<Measurement> {
        let _guard = self.lock.lock().await;

        let previous = self.clash.current_selection().await.unwrap_or_default();
        if previous != tag {
            self.clash
                .select(SELECTOR_TAG, tag)
                .await
                .map_err(|error| AgentError::Clash(error.to_string()))?;
        }

        let result = self.probe(tag, kind, url).await;

        let restored = if previous.is_empty() || previous == tag {
            previous.clone()
        } else if self.clash.select(SELECTOR_TAG, &previous).await.is_ok() {
            previous
        } else {
            tag.to_string()
        };

        let mut measurement = result?;
        measurement.restored = restored;
        Ok(measurement)
    }

    async fn probe(&self, tag: &str, kind: Kind, url: Option<&str>) -> Result<Measurement> {
        match kind {
            Kind::Latency => {
                let (elapsed, _, _) = self
                    .latency(url.unwrap_or(&self.config.latency_url))
                    .await?;
                Ok(Measurement::latency(tag, elapsed, ""))
            }
            Kind::Bandwidth => {
                let (_, bytes, seconds) = self
                    .bandwidth(url.unwrap_or(&self.config.download_url))
                    .await?;
                Ok(Measurement::bandwidth(tag, bytes, seconds, ""))
            }
            Kind::Both => {
                let (latency, _, _) = self
                    .latency(url.unwrap_or(&self.config.latency_url))
                    .await?;
                let bandwidth = self
                    .bandwidth(url.unwrap_or(&self.config.download_url))
                    .await
                    .ok()
                    .map(|(_, bytes, seconds)| (bytes, seconds));
                Ok(Measurement::combined(tag, latency, bandwidth, ""))
            }
        }
    }

    async fn latency(&self, url: &str) -> Result<(Duration, u64, f64)> {
        let started = Instant::now();
        let response = self
            .proxied
            .get(url)
            .send()
            .await
            .map_err(|error| AgentError::Measure(format!("request failed: {error}")))?;
        let elapsed = started.elapsed();
        if !response.status().is_success() {
            return Err(AgentError::Measure(format!(
                "probe returned HTTP {}",
                response.status()
            )));
        }
        Ok((elapsed, 0u64, 0.0))
    }

    async fn bandwidth(&self, url: &str) -> Result<(Duration, u64, f64)> {
        let started = Instant::now();
        let deadline = started + self.config.timeout;
        let mut response = self
            .proxied
            .get(url)
            .send()
            .await
            .map_err(|error| AgentError::Measure(format!("request failed: {error}")))?;

        if !response.status().is_success() {
            return Err(AgentError::Measure(format!(
                "download returned HTTP {}",
                response.status()
            )));
        }

        let mut bytes = 0u64;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| AgentError::Measure(format!("download failed: {error}")))?
        {
            bytes += chunk.len() as u64;
            if bytes >= self.config.max_bytes as u64 || Instant::now() >= deadline {
                break;
            }
        }

        let seconds = started.elapsed().as_secs_f64();
        if bytes == 0 {
            return Err(AgentError::Measure("download returned no data".into()));
        }
        Ok((Duration::from_secs_f64(seconds), bytes, seconds))
    }

    pub async fn serve(self: Arc<Self>, listen: SocketAddr) -> std::io::Result<()> {
        let router = Router::new()
            .route("/healthz", get(health))
            .route("/test/latency", post(latency))
            .route("/test/bandwidth", post(bandwidth))
            .route("/test/both", post(both))
            .layer(axum::middleware::from_fn(require_portal_header))
            .with_state(self);

        let listener = tokio::net::TcpListener::bind(listen).await?;
        tracing::info!(%listen, "agent test endpoint listening");
        axum::serve(listener, router).await
    }
}

async fn health(State(tester): State<Arc<Tester>>) -> impl IntoResponse {
    let selection = tester.clash.current_selection().await.unwrap_or_default();
    Json(json!({ "status": "ok", "selection": selection }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TestRequest {
    tag: String,
    url: Option<String>,
}

async fn latency(
    State(tester): State<Arc<Tester>>,
    Json(request): Json<TestRequest>,
) -> Response {
    run(&tester, request, Kind::Latency).await
}

async fn bandwidth(
    State(tester): State<Arc<Tester>>,
    Json(request): Json<TestRequest>,
) -> Response {
    run(&tester, request, Kind::Bandwidth).await
}

async fn both(State(tester): State<Arc<Tester>>, Json(request): Json<TestRequest>) -> Response {
    run(&tester, request, Kind::Both).await
}

async fn run(tester: &Tester, request: TestRequest, kind: Kind) -> Response {
    match tester
        .measure(&request.tag, kind, request.url.as_deref())
        .await
    {
        Ok(measurement) => Json(measurement).into_response(),
        Err(error) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": error.to_string() })),
        )
            .into_response(),
    }
}

async fn require_portal_header(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if request.method() == Method::GET {
        return next.run(request).await;
    }

    let allowed = request
        .headers()
        .get("x-portal-request")
        .and_then(|value| value.to_str().ok())
        == Some("1");

    if allowed {
        next.run(request).await
    } else {
        (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "missing X-Portal-Request header" })),
        )
            .into_response()
    }
}
