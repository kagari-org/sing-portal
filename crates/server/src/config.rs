use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use url::Url;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub listen: SocketAddr,
    pub db_path: PathBuf,
    pub sing_box: Option<PathBuf>,
    pub fetch_proxy: Option<String>,
    pub refresh_default: Duration,
    pub fetch_timeout: Duration,
    pub fetch_max_bytes: usize,
    pub static_dir: Option<PathBuf>,
    pub embed_web: bool,
    pub auto_interval: Duration,
    pub auto_candidates: usize,
    pub auto_margin: f64,
    pub score: sing_portal_core::score::ScoreConfig,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([127, 0, 0, 1], 3015)),
            db_path: PathBuf::from("portal.db"),
            sing_box: None,
            fetch_proxy: None,
            refresh_default: Duration::from_secs(6 * 3600),
            fetch_timeout: Duration::from_secs(20),
            fetch_max_bytes: 8 * 1024 * 1024,
            static_dir: None,
            embed_web: true,
            auto_interval: Duration::from_secs(10 * 60),
            auto_candidates: 5,
            auto_margin: 0.15,
            score: sing_portal_core::score::ScoreConfig::default(),
        }
    }
}

pub fn parse_duration(value: &str) -> Result<Duration, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("duration is empty".into());
    }
    let (number, unit) = value.split_at(value.len() - 1);
    if let Ok(seconds) = value.parse::<u64>() {
        return Ok(Duration::from_secs(seconds));
    }
    let seconds = number
        .parse::<u64>()
        .map_err(|_| format!("invalid duration `{value}`"))?;
    match unit {
        "s" => Ok(Duration::from_secs(seconds)),
        "m" => Ok(Duration::from_secs(seconds * 60)),
        "h" => Ok(Duration::from_secs(seconds * 3600)),
        "d" => Ok(Duration::from_secs(seconds * 86400)),
        other => Err(format!("unknown duration unit `{other}`")),
    }
}

pub fn mask_url(url: &str) -> String {
    match Url::parse(url) {
        Ok(parsed) => {
            let host = parsed.host_str().unwrap_or_default();
            let port = parsed.port().map(|port| format!(":{port}")).unwrap_or_default();
            format!("{}://{host}{port}/…", parsed.scheme())
        }
        Err(_) => "…".to_string(),
    }
}
