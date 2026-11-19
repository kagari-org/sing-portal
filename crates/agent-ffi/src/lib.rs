uniffi::setup_scaffolding!();

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use sing_portal_agent::state::LocalState;
use sing_portal_agent::tester::TesterConfig;
use sing_portal_agent::{Agent, AgentConfig, Host as AgentHost, HostError, HostResult};
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum FfiError {
    #[error("{message}")]
    Failed { message: String },
}

impl FfiError {
    fn new(message: impl ToString) -> Self {
        Self::Failed {
            message: message.to_string(),
        }
    }
}

/// One agent session in the host app. The agent's own state is handed to the
/// host as JSON, so the host decides how to persist it (DataStore, a file).
#[derive(uniffi::Record)]
pub struct AgentOptions {
    pub portal_url: String,
    pub node_name: String,
    pub token: Option<String>,
    /// Sing-box template as JSON text.
    pub template: String,
    pub api_url: String,
    pub latency_url: String,
    pub interval_secs: u64,
    pub health_timeout_secs: u64,
    pub best_effort: bool,
    /// Serve only the probes a person asked for; an automatic evaluation gets
    /// an error back instead of spending battery and data.
    pub manual_tests_only: bool,
    /// libbox's CommandServer secret, sent as `x-command-secret` metadata.
    pub api_secret: Option<String>,
}

#[derive(uniffi::Record)]
pub struct AgentStatus {
    pub revision: String,
    pub selection: String,
    pub health: String,
    pub error: String,
    pub connected: bool,
    pub connection_state: String,
    pub last_connected_at: i64,
    pub last_sync_at: i64,
    pub retry_in_secs: i64,
    pub connection_error: String,
}

#[uniffi::export(with_foreign)]
pub trait Host: Send + Sync {
    fn write_config(&self, config: Vec<u8>) -> Result<(), FfiError>;
    fn has_config(&self) -> bool;
    fn restart(&self) -> Result<(), FfiError>;
    fn load_state(&self) -> Option<String>;
    fn save_state(&self, state: String) -> Result<(), FfiError>;
}

/// Bridges the foreign trait onto the agent's own, keeping the agent state as
/// opaque JSON so no protocol types leak into the bindings.
struct Bridge {
    host: Arc<dyn Host>,
}

impl AgentHost for Bridge {
    fn write_config(&self, config: &[u8]) -> HostResult<()> {
        self.host
            .write_config(config.to_vec())
            .map_err(|error| HostError::new(error.to_string()))
    }

    fn has_config(&self) -> bool {
        self.host.has_config()
    }

    fn restart(&self) -> HostResult<()> {
        self.host
            .restart()
            .map_err(|error| HostError::new(error.to_string()))
    }

    fn load_state(&self) -> Option<LocalState> {
        let text = self.host.load_state()?;
        serde_json::from_str(&text).ok()
    }

    fn save_state(&self, state: &LocalState) -> HostResult<()> {
        let text =
            serde_json::to_string(state).map_err(|error| HostError::new(error.to_string()))?;
        self.host
            .save_state(text)
            .map_err(|error| HostError::new(error.to_string()))
    }
}

#[derive(uniffi::Object)]
pub struct PortalAgent {
    agent: Arc<Agent>,
    bridge: Arc<Bridge>,
    running: Mutex<Option<JoinHandle<()>>>,
}

#[uniffi::export]
impl PortalAgent {
    #[uniffi::constructor]
    pub fn new(options: AgentOptions, host: Arc<dyn Host>) -> Result<Arc<Self>, FfiError> {
        // the api channel is built lazily and registers a reactor, so
        // construction has to happen inside the runtime whichever thread the
        // host app calls this from
        let _guard = runtime().enter();

        let bridge = Arc::new(Bridge { host });
        let config = AgentConfig {
            portal_url: options.portal_url,
            node_name: options.node_name,
            token: options.token,
            template: options.template,
            api_url: options.api_url,
            api_secret: options.api_secret,
            latency_url: options.latency_url,
            interval: Duration::from_secs(options.interval_secs.max(1)),
            once: false,
            best_effort: options.best_effort,
            auto_measure: !options.manual_tests_only,
            health_timeout: Duration::from_secs(options.health_timeout_secs.max(1)),
            egress_health_interval: Duration::ZERO,
            egress_health_failures: 2,
            tester: TesterConfig::default(),
        };

        let agent = Agent::new(config, bridge.clone()).map_err(FfiError::new)?;
        Ok(Arc::new(Self {
            agent: Arc::new(agent),
            bridge,
            running: Mutex::new(None),
        }))
    }

    /// Connects and keeps the session alive until `stop`, retrying on its own.
    pub fn start(&self) {
        let mut running = self.running.lock().expect("running lock");
        if running.is_some() {
            return;
        }

        self.agent.runtime_status().set_connecting();
        let agent = self.agent.clone();
        let handle = runtime().spawn(async move {
            if let Err(error) = agent.run().await {
                tracing::warn!(%error, "agent stopped");
            }
        });
        *running = Some(handle);
    }

    pub fn stop(&self) {
        let mut running = self.running.lock().expect("running lock");
        if let Some(handle) = running.take() {
            handle.abort();
        }
        self.agent.runtime_status().set_stopped();
    }

    pub fn is_running(&self) -> bool {
        self.running
            .lock()
            .expect("running lock")
            .as_ref()
            .is_some_and(|handle| !handle.is_finished())
    }

    /// Writes the initial config (if the host has none) without starting the
    /// session. Callers run this on an IO thread, not from a host callback.
    pub fn prepare(&self) -> Result<bool, FfiError> {
        runtime()
            .block_on(self.agent.ensure_config())
            .map_err(FfiError::new)?;
        Ok(AgentHost::has_config(&*self.bridge))
    }

    /// What the agent last committed, read from the host's own state, plus what
    /// the session loop is doing. This never talks to sing-box, so it is safe
    /// to call from the UI thread.
    pub fn status(&self) -> AgentStatus {
        let state = AgentHost::load_state(&*self.bridge).unwrap_or_default();
        let runtime = self.agent.runtime_status();
        let last_sync_at = state.updated_at;
        let health = if state.last_error.is_empty() {
            "ok"
        } else {
            "degraded"
        };
        AgentStatus {
            revision: state.applied_revision,
            selection: state.applied_selection,
            health: health.to_string(),
            error: state.last_error,
            connected: runtime.connected(),
            connection_state: runtime.state().as_str().to_string(),
            last_connected_at: runtime.last_connected_at(),
            last_sync_at,
            retry_in_secs: runtime.retry_in_secs(),
            connection_error: runtime.connection_error(),
        }
    }
}

fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("agent runtime")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeHost {
        config: Mutex<Option<Vec<u8>>>,
        state: Mutex<Option<String>>,
    }

    impl Host for FakeHost {
        fn write_config(&self, config: Vec<u8>) -> Result<(), FfiError> {
            *self.config.lock().unwrap() = Some(config);
            Ok(())
        }

        fn has_config(&self) -> bool {
            self.config.lock().unwrap().is_some()
        }

        fn restart(&self) -> Result<(), FfiError> {
            Ok(())
        }

        fn load_state(&self) -> Option<String> {
            self.state.lock().unwrap().clone()
        }

        fn save_state(&self, state: String) -> Result<(), FfiError> {
            *self.state.lock().unwrap() = Some(state);
            Ok(())
        }
    }

    fn options() -> AgentOptions {
        AgentOptions {
            portal_url: "http://127.0.0.1:1".into(),
            node_name: "phone".into(),
            token: None,
            template: r#"{"log":{"level":"warn"}}"#.into(),
            api_url: "http://127.0.0.1:1".into(),
            latency_url: "http://127.0.0.1:1/".into(),
            interval_secs: 15,
            health_timeout_secs: 1,
            best_effort: true,
            manual_tests_only: true,
            api_secret: None,
        }
    }

    #[test]
    fn reports_the_state_its_host_stored() {
        let host = Arc::new(FakeHost::default());
        *host.state.lock().unwrap() = Some(
            r#"{"appliedRevision":"sha256:x","appliedSelection":"node-a","lastError":""}"#.into(),
        );

        let agent = PortalAgent::new(options(), host).unwrap();
        let status = agent.status();
        assert_eq!(status.revision, "sha256:x");
        assert_eq!(status.selection, "node-a");
        assert_eq!(status.health, "ok");
        assert!(status.error.is_empty());
        assert!(!status.connected);
        assert_eq!(status.connection_state, "stopped");
        assert_eq!(status.last_connected_at, 0);
        assert_eq!(status.retry_in_secs, 0);
        assert!(status.connection_error.is_empty());
        assert!(!agent.is_running());
    }

    #[test]
    fn reports_the_last_sync_timestamp() {
        let host = Arc::new(FakeHost::default());
        *host.state.lock().unwrap() =
            Some(r#"{"appliedRevision":"sha256:x","updatedAt":1700000000}"#.into());

        let agent = PortalAgent::new(options(), host).unwrap();
        assert_eq!(agent.status().last_sync_at, 1_700_000_000);
    }

    #[test]
    fn prepare_writes_initial_config() {
        let host = Arc::new(FakeHost::default());
        let agent = PortalAgent::new(options(), host.clone()).unwrap();

        assert!(!host.has_config());
        assert!(agent.prepare().unwrap());
        assert!(host.has_config());
    }

    #[test]
    fn start_and_stop_are_idempotent() {
        let host = Arc::new(FakeHost::default());
        let agent = PortalAgent::new(options(), host).unwrap();

        agent.start();
        assert!(agent.is_running());
        agent.start();
        assert!(agent.is_running());

        agent.stop();
        assert!(!agent.is_running());
        agent.stop();
    }
}
