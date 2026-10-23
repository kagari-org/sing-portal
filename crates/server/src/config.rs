use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use url::Url;

#[derive(Debug, Clone)]
pub struct NodeConfig {
    pub name: String,
    pub address: String,
}

impl NodeConfig {
    pub fn host(&self) -> String {
        let raw = self
            .address
            .trim_start_matches("http://")
            .trim_start_matches("https://");
        match raw.rsplit_once(':') {
            Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => host
                .trim_start_matches('[')
                .trim_end_matches(']')
                .to_string(),
            _ => raw.to_string(),
        }
    }

    pub fn clash_base_url(&self) -> String {
        if self.address.starts_with("http://") || self.address.starts_with("https://") {
            self.address.clone()
        } else {
            format!("http://{}", self.address)
        }
    }
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub listen: SocketAddr,
    pub db_path: PathBuf,
    pub nodes: Vec<NodeConfig>,
    pub sing_box: Option<PathBuf>,
    pub fetch_proxy: Option<String>,
    pub test_port: u16,
    pub refresh_default: Duration,
    pub fetch_timeout: Duration,
    pub fetch_max_bytes: usize,
    pub static_dir: Option<PathBuf>,
    pub embed_web: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([127, 0, 0, 1], 3015)),
            db_path: PathBuf::from("portal.db"),
            nodes: Vec::new(),
            sing_box: None,
            fetch_proxy: None,
            test_port: 3016,
            refresh_default: Duration::from_secs(6 * 3600),
            fetch_timeout: Duration::from_secs(20),
            fetch_max_bytes: 8 * 1024 * 1024,
            static_dir: None,
            embed_web: true,
        }
    }
}

pub fn parse_nodes(value: &str) -> Result<Vec<NodeConfig>, String> {
    let mut nodes = Vec::new();
    for entry in value.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let (name, address) = entry
            .split_once('=')
            .ok_or_else(|| format!("node `{entry}` must look like name=host:port"))?;
        let name = name.trim();
        let address = address.trim();
        if name.is_empty() || address.is_empty() {
            return Err(format!("node `{entry}` must look like name=host:port"));
        }
        if !nodes.iter().any(|node: &NodeConfig| node.name == name) {
            nodes.push(NodeConfig {
                name: name.to_string(),
                address: address.to_string(),
            });
        }
    }
    Ok(nodes)
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
