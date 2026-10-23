pub mod api;
pub mod apply;
pub mod config;
pub mod error;
pub mod state;
pub mod tester;

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use sing_portal_core::protocol::{AgentMessage, ServerMessage};
use sing_portal_core::render::SELECTOR_TAG;
use tokio_tungstenite::tungstenite::Message;
use url::Url;

pub use config::AgentConfig;
pub use error::{AgentError, Result};
use api::BoxApi;

const RENDERER_VERSION: u32 = 2;
use tester::{Kind, Tester};

pub struct Agent {
    config: AgentConfig,
    api: BoxApi,
    tester: Tester,
}

impl Agent {
    pub fn new(config: AgentConfig) -> Result<Self> {
        let api = BoxApi::new(&config.api_url, Duration::from_secs(5))?;
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
        let Ok(bytes) = apply::render_bytes(&self.config, &[]).await else {
            return;
        };
        if let Err(error) = state::write_atomic(&self.config.config_path, &bytes).await {
            tracing::warn!(%error, "cannot write the initial config");
        }
    }

    async fn session(&self) -> Result<()> {
        let url = websocket_url(&self.config.portal_url)?;
        let (socket, _) = tokio_tungstenite::connect_async(url.as_str())
            .await
            .map_err(|error| AgentError::Portal(error.to_string()))?;
        let (mut sink, mut stream) = socket.split();

        let (sender, mut receiver) = tokio::sync::mpsc::channel::<Message>(64);
        let writer = tokio::spawn(async move {
            while let Some(message) = receiver.recv().await {
                if sink.send(message).await.is_err() {
                    break;
                }
            }
        });

        let hello = AgentMessage::Hello {
            name: self.config.node_name.clone(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        };
        send_message(&sender, &hello).await?;
        tracing::info!(portal = %url, "agent connected");

        let mut ticker = tokio::time::interval(self.config.interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await;

        let result = loop {
            tokio::select! {
                frame = stream.next() => {
                    match frame {
                        Some(Ok(Message::Text(text))) => {
                            let message: ServerMessage = serde_json::from_str(&text)?;
                            if let ServerMessage::Test { id, kind, tag } = message {
                                spawn_test(self.tester.clone(), sender.clone(), id, kind, tag);
                                continue;
                            }
                            let is_sync = matches!(message, ServerMessage::Sync { .. });
                            let reply = self.handle(message).await;
                            send_message(&sender, &reply).await?;
                            if is_sync && self.config.once {
                                break Ok(());
                            }
                        }
                        Some(Ok(Message::Close(_))) | None => break Ok(()),
                        Some(Err(error)) => break Err(AgentError::Portal(error.to_string())),
                        _ => {}
                    }
                }
                _ = ticker.tick() => {
                    let status = self.status().await;
                    send_message(&sender, &status).await?;
                }
            }
        };

        drop(sender);
        let _ = writer.await;
        result
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
        // the rendered config embeds the latency url; re-render when it changes
        let latency_url_changed = local.applied_latency_url != self.config.latency_url;
        // the agent controls parts of the rendered config (test inbound), so a
        // renderer change must be re-applied even when the revision is stable
        let renderer_changed = local.applied_renderer_version != RENDERER_VERSION;
        if empty {
            health = "no_active_subscriptions".to_string();
        } else if revision != local.applied_revision
            || config_missing
            || latency_url_changed
            || renderer_changed
        {
            match apply::apply(&self.config, &outbounds, &self.api).await {
                Ok(()) => {
                    local.applied_revision = revision.clone();
                    local.applied_latency_url = self.config.latency_url.clone();
                    local.applied_renderer_version = RENDERER_VERSION;
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

async fn send_message(sender: &tokio::sync::mpsc::Sender<Message>, message: &AgentMessage) -> Result<()> {
    sender
        .send(Message::Text(serialize(message)?.into()))
        .await
        .map_err(|_| AgentError::Portal("websocket writer closed".into()))
}

fn spawn_test(
    tester: Tester,
    sender: tokio::sync::mpsc::Sender<Message>,
    id: String,
    kind: String,
    tag: String,
) {
    tokio::spawn(async move {
        let message = match Kind::parse(&kind) {
            Some(kind) => match tester.measure(&tag, kind).await {
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
            },
            None => AgentMessage::Error {
                id: Some(id),
                message: format!("unknown test kind `{kind}`"),
            },
        };
        if let Ok(text) = serialize(&message) {
            let _ = sender.send(Message::Text(text.into())).await;
        }
    });
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
