pub use sing_portal_core::parse_duration;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use url::Url;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub listen: SocketAddr,
    pub db_path: PathBuf,
    pub fetch_proxy: Option<String>,
    pub refresh_default: Duration,
    pub fetch_timeout: Duration,
    pub fetch_max_bytes: usize,
    pub static_dir: Option<PathBuf>,
    pub embed_web: bool,
    /// Separate listener for the agent endpoint. When unset the agent route
    /// stays on `listen`.
    pub agent_listen: Option<SocketAddr>,
    pub agent_token: Option<String>,
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
            fetch_proxy: None,
            refresh_default: Duration::from_secs(6 * 3600),
            fetch_timeout: Duration::from_secs(20),
            fetch_max_bytes: 8 * 1024 * 1024,
            static_dir: None,
            embed_web: true,
            agent_listen: None,
            agent_token: None,
            auto_interval: Duration::from_secs(2 * 3600),
            auto_candidates: 6,
            auto_margin: 0.15,
            score: sing_portal_core::score::ScoreConfig::default(),
        }
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
