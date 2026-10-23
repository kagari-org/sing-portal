use std::collections::HashSet;
use std::sync::Arc;

use reqwest::redirect::Policy;
use tokio::sync::Mutex;

use crate::config::{NodeConfig, ServerConfig};
use sing_portal_store::{Store, StoreError};

#[derive(Clone)]
pub struct AppState {
    config: Arc<ServerConfig>,
    store: Store,
    http: reqwest::Client,
    test_http: reqwest::Client,
    proxy_http: Option<reqwest::Client>,
    refreshing: Arc<Mutex<HashSet<String>>>,
}

impl AppState {
    pub async fn new(config: ServerConfig) -> Result<Self, StoreError> {
        let store = Store::connect(&config.db_path).await?;

        let http = reqwest::Client::builder()
            .timeout(config.fetch_timeout)
            .redirect(Policy::limited(3))
            .build()
            .expect("http client");

        let test_http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(90))
            .build()
            .expect("test http client");

        let proxy_http = match &config.fetch_proxy {
            Some(proxy) => match reqwest::Proxy::all(proxy) {
                Ok(proxy) => Some(
                    reqwest::Client::builder()
                        .timeout(config.fetch_timeout)
                        .redirect(Policy::limited(3))
                        .proxy(proxy)
                        .build()
                        .expect("proxy http client"),
                ),
                Err(err) => {
                    tracing::warn!(%proxy, error = %err, "ignoring invalid fetch proxy");
                    None
                }
            },
            None => None,
        };

        Ok(Self {
            config: Arc::new(config),
            store,
            http,
            test_http,
            proxy_http,
            refreshing: Arc::new(Mutex::new(HashSet::new())),
        })
    }

    pub fn config(&self) -> &ServerConfig {
        &self.config
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub fn test_http(&self) -> &reqwest::Client {
        &self.test_http
    }

    pub fn proxy_http(&self) -> Option<&reqwest::Client> {
        self.proxy_http.as_ref()
    }

    pub fn node(&self, name: &str) -> Option<&NodeConfig> {
        self.config.nodes.iter().find(|node| node.name == name)
    }

    pub async fn begin_refresh(&self, id: &str) -> bool {
        self.refreshing.lock().await.insert(id.to_string())
    }

    pub async fn end_refresh(&self, id: &str) {
        self.refreshing.lock().await.remove(id);
    }
}
