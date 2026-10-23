use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use sing_portal_core::protocol::{AgentMessage, ServerMessage};
use tokio::sync::{mpsc, oneshot, Mutex};

use crate::error::{ApiError, ApiResult};

#[derive(Clone)]
struct ConnectedAgent {
    outbound: mpsc::Sender<ServerMessage>,
    version: String,
}

#[derive(Default)]
pub struct Registry {
    agents: Mutex<HashMap<String, ConnectedAgent>>,
    pending: Mutex<HashMap<String, oneshot::Sender<AgentMessage>>>,
}

impl Registry {
    pub async fn register(
        &self,
        name: &str,
        version: &str,
        outbound: mpsc::Sender<ServerMessage>,
    ) {
        self.agents.lock().await.insert(
            name.to_string(),
            ConnectedAgent {
                outbound,
                version: version.to_string(),
            },
        );
    }

    pub async fn unregister(&self, name: &str) {
        self.agents.lock().await.remove(name);
    }

    pub async fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.agents.lock().await.keys().cloned().collect();
        names.sort();
        names
    }

    pub async fn is_connected(&self, name: &str) -> bool {
        self.agents.lock().await.contains_key(name)
    }

    pub async fn version(&self, name: &str) -> Option<String> {
        self.agents
            .lock()
            .await
            .get(name)
            .map(|agent| agent.version.clone())
    }

    pub async fn send(&self, name: &str, message: ServerMessage) -> ApiResult<()> {
        let outbound = {
            let agents = self.agents.lock().await;
            agents.get(name).map(|agent| agent.outbound.clone())
        };
        let Some(outbound) = outbound else {
            return Err(ApiError::Upstream(format!("agent `{name}` is not connected")));
        };

        outbound
            .send(message)
            .await
            .map_err(|_| ApiError::Upstream(format!("agent `{name}` disconnected")))
    }

    pub async fn request(
        &self,
        name: &str,
        message: ServerMessage,
        timeout: Duration,
    ) -> ApiResult<AgentMessage> {
        let id = message.id().to_string();
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().await.insert(id.clone(), sender);

        if let Err(error) = self.send(name, message).await {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }

        match tokio::time::timeout(timeout, receiver).await {
            Ok(Ok(message)) => Ok(message),
            Ok(Err(_)) => Err(ApiError::Upstream(format!(
                "agent `{name}` dropped the request"
            ))),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(ApiError::Upstream(format!(
                    "agent `{name}` timed out after {}s",
                    timeout.as_secs()
                )))
            }
        }
    }

    pub async fn resolve(&self, id: &str, message: AgentMessage) -> bool {
        let sender = self.pending.lock().await.remove(id);
        match sender {
            Some(sender) => sender.send(message).is_ok(),
            None => false,
        }
    }
}

pub type SharedRegistry = Arc<Registry>;
