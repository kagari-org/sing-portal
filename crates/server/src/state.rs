use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use reqwest::redirect::Policy;
use tokio::sync::{Mutex, broadcast};

use crate::config::ServerConfig;
use crate::evaluation::{EvaluationProgress, ServerEvent};
use crate::registry::{Registry, SharedRegistry};
use sing_portal_store::{Store, StoreError};

#[derive(Clone)]
pub struct AppState {
    config: Arc<ServerConfig>,
    store: Store,
    http: reqwest::Client,
    proxy_http: Option<reqwest::Client>,
    refreshing: Arc<Mutex<HashSet<String>>>,
    auto_running: Arc<Mutex<HashSet<String>>>,
    evaluations: Arc<Mutex<HashMap<String, EvaluationProgress>>>,
    evaluation_seq: Arc<AtomicU64>,
    events: broadcast::Sender<ServerEvent>,
    registry: SharedRegistry,
}

impl AppState {
    pub async fn new(config: ServerConfig) -> Result<Self, StoreError> {
        let store = Store::connect(&config.db_path).await?;

        let http = reqwest::Client::builder()
            .timeout(config.fetch_timeout)
            .redirect(Policy::limited(3))
            .build()
            .expect("http client");

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
                Err(error) => {
                    tracing::warn!(%proxy, %error, "ignoring invalid fetch proxy");
                    None
                }
            },
            None => None,
        };

        let (events, _) = broadcast::channel(256);

        Ok(Self {
            config: Arc::new(config),
            store,
            http,
            proxy_http,
            refreshing: Arc::new(Mutex::new(HashSet::new())),
            auto_running: Arc::new(Mutex::new(HashSet::new())),
            evaluations: Arc::new(Mutex::new(HashMap::new())),
            evaluation_seq: Arc::new(AtomicU64::new(0)),
            events,
            registry: Arc::new(Registry::default()),
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

    pub fn proxy_http(&self) -> Option<&reqwest::Client> {
        self.proxy_http.as_ref()
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<ServerEvent> {
        self.events.subscribe()
    }

    pub fn notify_changed(&self, scopes: &[&str]) {
        let _ = self.events.send(ServerEvent::Changed {
            scopes: scopes.iter().map(|scope| scope.to_string()).collect(),
        });
    }

    fn emit_evaluation(&self, evaluation: &EvaluationProgress) {
        let _ = self.events.send(ServerEvent::Evaluation {
            evaluation: evaluation.clone(),
        });
    }

    pub async fn evaluation(&self, node: &str) -> Option<EvaluationProgress> {
        self.evaluations.lock().await.get(node).cloned()
    }

    pub async fn begin_auto(&self, node: &str) -> bool {
        if !self.auto_running.lock().await.insert(node.to_string()) {
            return false;
        }

        let run_id = self.evaluation_seq.fetch_add(1, Ordering::SeqCst) + 1;
        let evaluation = EvaluationProgress {
            machine: node.to_string(),
            run_id,
            running: true,
            phase: "starting".to_string(),
            done: 0,
            total: 0,
            current: None,
            started_at: sing_portal_store::now(),
            finished_at: None,
            best: None,
            score: None,
            switched: false,
            checked: 0,
            error: String::new(),
        };
        self.evaluations
            .lock()
            .await
            .insert(node.to_string(), evaluation.clone());
        self.emit_evaluation(&evaluation);
        true
    }

    pub async fn update_evaluation(
        &self,
        node: &str,
        phase: &str,
        done: usize,
        total: usize,
        current: Option<&str>,
    ) {
        let evaluation = {
            let mut evaluations = self.evaluations.lock().await;
            let Some(evaluation) = evaluations.get_mut(node) else {
                return;
            };
            evaluation.phase = phase.to_string();
            evaluation.done = done;
            evaluation.total = total;
            evaluation.current = current.map(ToString::to_string);
            evaluation.clone()
        };
        self.emit_evaluation(&evaluation);
    }

    pub async fn finish_evaluation(
        &self,
        node: &str,
        best: Option<String>,
        score: Option<f64>,
        switched: bool,
        checked: usize,
        error: &str,
    ) {
        let evaluation = {
            let mut evaluations = self.evaluations.lock().await;
            let Some(evaluation) = evaluations.get_mut(node) else {
                return;
            };
            evaluation.running = false;
            evaluation.phase = if error.is_empty() { "done" } else { "error" }.to_string();
            evaluation.current = None;
            evaluation.finished_at = Some(sing_portal_store::now());
            evaluation.best = best;
            evaluation.score = score;
            evaluation.switched = switched;
            evaluation.checked = checked;
            evaluation.error = error.to_string();
            evaluation.clone()
        };
        self.emit_evaluation(&evaluation);
    }

    pub async fn end_auto(&self, node: &str) {
        self.auto_running.lock().await.remove(node);

        let evaluation = {
            let mut evaluations = self.evaluations.lock().await;
            let Some(evaluation) = evaluations.get_mut(node) else {
                return;
            };
            if evaluation.running {
                evaluation.running = false;
                evaluation.phase = "done".to_string();
                evaluation.finished_at = Some(sing_portal_store::now());
            }
            evaluation.clone()
        };
        self.emit_evaluation(&evaluation);
    }

    pub async fn begin_refresh(&self, id: &str) -> bool {
        self.refreshing.lock().await.insert(id.to_string())
    }

    pub async fn end_refresh(&self, id: &str) {
        self.refreshing.lock().await.remove(id);
    }
}
