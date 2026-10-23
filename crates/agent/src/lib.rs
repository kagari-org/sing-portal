pub mod api;
pub mod apply;
pub mod config;
pub mod error;
pub mod state;
pub mod tester;

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use sing_portal_core::protocol::{AgentMessage, ServerMessage};
use sing_portal_core::render::{render, to_bytes};
use sing_portal_core::render::SELECTOR_TAG;
use tokio_tungstenite::tungstenite::Message;
use url::Url;

pub use config::AgentConfig;
pub use error::{AgentError, Result};
use api::BoxApi;
use tester::{Kind, Tester};

pub struct Agent {
    config: AgentConfig,
    api: BoxApi,
    tester: Tester,
}

impl Agent {
    pub fn new(config: AgentConfig) -> Result<Self> {
        let api = BoxApi::new(&config.sing_box_bin, config.api_url.clone(), Duration::from_secs(5));
        let tester = Tester::new(config.tester.clone(), api.clone());
        Ok(Self { config, api, tester })
    }

    pub fn config(&self) -> &AgentConfig {
        &self.config
    }

    pub async fn run(self) -> Result<()> {
        self.ensure_config().await;

        if self.config.once {
            return match self.session().await {
                Ok(()) => Ok(()),
                Err(error) if self.config.best_effort => {
                    tracing::warn!(%error, "agent run failed, ignoring because of --best-effort");
                    Ok(())
                }
                Err(error) => Err(error),
            };
        }

        let mut backoff = Duration::from_secs(1);
        loop {
            match self.session().await {
                Ok(()) => {
                    tracing::info!("agent session closed");
                    backoff = Duration::from_secs(1);
                }
                Err(error) => tracing::warn!(%error, "agent session failed"),
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    }

    async fn ensure_config(&self) {
        if tokio::fs::try_exists(&self.config.config_path)
            .await
            .unwrap_or(false)
        {
            return;
        }
        let Ok(template_bytes) = tokio::fs::read(&self.config.template_path).await else {
            return;
        };
        let Ok(template) = serde_json::from_slice::<serde_json::Value>(&template_bytes) else {
            return;
        };
        let Ok(config) = render(&template, &[]) else {
            return;
        };
        if let Err(error) = state::write_atomic(&self.config.config_path, &to_bytes(&config)).await {
            tracing::warn!(%error, "cannot write the initial config");
        }
    }

    async fn session(&self) -> Result<()> {
        let url = websocket_url(&self.config.portal_url)?;
        let (socket, _) = tokio_tungstenite::connect_async(url.as_str())
            .await
            .map_err(|error| AgentError::Portal(error.to_string()))?;
        let (mut sink, mut stream) = socket.split();

        let hello = AgentMessage::Hello {
            name: self.config.node_name.clone(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        };
        sink.send(Message::Text(serialize(&hello)?.into()))
            .await
            .map_err(|error| AgentError::Portal(error.to_string()))?;
        tracing::info!(portal = %url, "agent connected");

        let mut ticker = tokio::time::interval(self.config.interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await;

        loop {
            tokio::select! {
                frame = stream.next() => {
                    match frame {
                        Some(Ok(Message::Text(text))) => {
                            let message: ServerMessage = serde_json::from_str(&text)?;
                            let is_sync = matches!(message, ServerMessage::Sync { .. });
                            let reply = self.handle(message).await;
                            sink.send(Message::Text(serialize(&reply)?.into()))
                                .await
                                .map_err(|error| AgentError::Portal(error.to_string()))?;
                            if is_sync && self.config.once {
                                return Ok(());
                            }
                        }
                        Some(Ok(Message::Close(_))) | None => return Ok(()),
                        Some(Err(error)) => return Err(AgentError::Portal(error.to_string())),
                        _ => {}
                    }
                }
                _ = ticker.tick() => {
                    let status = self.status().await;
                    sink.send(Message::Text(serialize(&status)?.into()))
                        .await
                        .map_err(|error| AgentError::Portal(error.to_string()))?;
                }
            }
        }
    }

    async fn handle(&self, message: ServerMessage) -> AgentMessage {
        match message {
            ServerMessage::Sync {
                id,
                revision,
                selection_revision: _,
                selection,
                outbounds,
            } => self.sync(id, revision, selection, outbounds).await,
            ServerMessage::Test { id, kind, tag } => {
                let Some(kind) = Kind::parse(&kind) else {
                    return AgentMessage::Error {
                        id: Some(id),
                        message: format!("unknown test kind `{kind}`"),
                    };
                };
                match self.tester.measure(&tag, kind).await {
                    Ok(measurement) => AgentMessage::TestResult {
                        id,
                        kind: measurement.kind,
                        latency_ms: measurement.latency_ms,
                        down_mbps: measurement.down_mbps,
                        up_mbps: measurement.up_mbps,
                        responsiveness: measurement.responsiveness,
                    },
                    Err(error) => AgentMessage::Error {
                        id: Some(id),
                        message: error.to_string(),
                    },
                }
            }
            ServerMessage::Select { id, outbound } => {
                match self.api.select(SELECTOR_TAG, &outbound).await {
                    Ok(()) => AgentMessage::SelectResult { id, applied: true },
                    Err(error) => AgentMessage::Error {
                        id: Some(id),
                        message: error.to_string(),
                    },
                }
            }
        }
    }

    async fn sync(
        &self,
        id: String,
        revision: String,
        selection: String,
        outbounds: Vec<serde_json::Value>,
    ) -> AgentMessage {
        let mut local = state::load(&self.config.local_state_path()).await;
        let mut health = "ok".to_string();
        let mut error = String::new();
        let mut applied = false;

        let empty = revision == "none" || outbounds.is_empty();
        let config_missing = !tokio::fs::try_exists(&self.config.config_path)
            .await
            .unwrap_or(false);
        if empty {
            health = "no_active_subscriptions".to_string();
        } else if revision != local.applied_revision || config_missing {
            match apply::apply(&self.config, &outbounds, &self.api).await {
                Ok(()) => {
                    local.applied_revision = revision.clone();
                    applied = true;
                }
                Err(apply_error) => {
                    health = "rollback".to_string();
                    error = apply_error.to_string();
                }
            }
        }

        if health == "ok" || empty {
            let tags = apply::tags_of(&outbounds);
            match apply::apply_selection(&self.api, &selection, &tags).await {
                Ok(selection) => local.applied_selection = selection,
                Err(selection_error) => {
                    health = "degraded".to_string();
                    error = selection_error.to_string();
                }
            }
        } else {
            local.applied_selection = selection;
        }

        local.last_error = error.clone();
        local.updated_at = unix_now();
        if let Err(save_error) = state::save(&self.config.local_state_path(), &local).await {
            tracing::warn!(%save_error, "cannot persist agent state");
        }

        AgentMessage::SyncResult {
            id,
            revision: local.applied_revision,
            selection: local.applied_selection,
            health,
            error,
            applied,
        }
    }

    async fn status(&self) -> AgentMessage {
        let local = state::load(&self.config.local_state_path()).await;
        let selection = self
            .api
            .current_selection()
            .await
            .unwrap_or_else(|_| local.applied_selection.clone());
        let health = if local.last_error.is_empty() {
            "ok"
        } else {
            "degraded"
        };

        AgentMessage::Status {
            revision: local.applied_revision,
            selection,
            health: health.to_string(),
            error: local.last_error,
        }
    }
}

fn serialize(message: &AgentMessage) -> Result<String> {
    Ok(serde_json::to_string(message)?)
}

fn websocket_url(portal: &str) -> Result<Url> {
    let mut url = Url::parse(portal)
        .map_err(|error| AgentError::Portal(format!("invalid portal url: {error}")))?;

    let current = url.scheme().to_string();
    let scheme = match current.as_str() {
        "http" => "ws",
        "https" => "wss",
        "ws" | "wss" => current.as_str(),
        other => {
            return Err(AgentError::Portal(format!(
                "unsupported portal scheme `{other}`"
            )));
        }
    };
    if scheme != current {
        url.set_scheme(scheme)
            .map_err(|()| AgentError::Portal("cannot set websocket scheme".into()))?;
    }

    if url.path().is_empty() || url.path() == "/" {
        url.set_path("/api/v1/agent/ws");
    }
    url.set_query(None);
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::websocket_url;

    #[test]
    fn accepts_ws_and_http_portal_urls() {
        assert_eq!(
            websocket_url("ws://10.11.0.3:3015").unwrap().as_str(),
            "ws://10.11.0.3:3015/api/v1/agent/ws"
        );
        assert_eq!(
            websocket_url("wss://portal.example.com").unwrap().as_str(),
            "wss://portal.example.com/api/v1/agent/ws"
        );
        assert_eq!(
            websocket_url("http://10.11.0.3:3015").unwrap().as_str(),
            "ws://10.11.0.3:3015/api/v1/agent/ws"
        );
        assert_eq!(
            websocket_url("ws://10.11.0.3:3015/custom/ws")
                .unwrap()
                .as_str(),
            "ws://10.11.0.3:3015/custom/ws"
        );
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}
