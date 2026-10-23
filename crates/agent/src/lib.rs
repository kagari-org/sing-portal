pub mod apply;
pub mod config;
pub mod error;
pub mod portal;
pub mod state;

use std::time::Duration;

use sing_portal_core::canonical::selection_revision;
use sing_portal_core::clash_api::ClashClient;
use sing_portal_core::render::AUTO_TAG;

pub use config::AgentConfig;
pub use error::{AgentError, Result};
use portal::{PortalClient, Report};

#[derive(Debug, Clone)]
pub struct Outcome {
    pub revision: String,
    pub selection: String,
    pub health: String,
    pub error: String,
    pub applied: bool,
}

pub struct Agent {
    config: AgentConfig,
    portal: PortalClient,
    clash: ClashClient,
}

impl Agent {
    pub fn new(config: AgentConfig) -> Result<Self> {
        let portal = PortalClient::new(&config.portal_url, &config.node_name)?;
        let clash = ClashClient::new(&config.clash_url, Duration::from_secs(5))
            .map_err(|error| AgentError::Clash(error.to_string()))?;
        Ok(Self {
            config,
            portal,
            clash,
        })
    }

    pub fn config(&self) -> &AgentConfig {
        &self.config
    }

    pub async fn run_once(&self) -> Result<Outcome> {
        let mut local = state::load(&self.config.local_state_path()).await;
        let cached_selection_revision = if local.applied_selection.is_empty() {
            None
        } else {
            Some(selection_revision(
                &self.config.node_name,
                &local.applied_selection,
            ))
        };

        let fetched = self
            .portal
            .fetch_state(
                Some(&local.applied_revision),
                cached_selection_revision.as_deref(),
            )
            .await;

        let mut health = "ok".to_string();
        let mut error = String::new();
        let mut applied = false;

        match fetched {
            Ok(None) => {
                if let Err(selection_error) = self.apply_selection(&local.applied_selection).await {
                    health = "degraded".into();
                    error = selection_error.to_string();
                }
            }
            Ok(Some(node_state)) => {
                if node_state.revision == "none" || node_state.outbounds.is_empty() {
                    health = "no_active_subscriptions".into();
                } else {
                    match apply::apply(&self.config, &node_state.outbounds, &self.clash).await {
                        Ok(()) => {
                            local.applied_revision = node_state.revision.clone();
                            applied = true;
                        }
                        Err(apply_error) => {
                            health = "rollback".into();
                            error = apply_error.to_string();
                        }
                    }
                }

                if health == "ok" || health == "no_active_subscriptions" {
                    let tags = apply::tags_of(&node_state.outbounds);
                    match apply::apply_selection(&self.clash, &node_state.selection, &tags).await {
                        Ok(selection) => local.applied_selection = selection,
                        Err(selection_error) => {
                            health = "degraded".into();
                            error = selection_error.to_string();
                        }
                    }
                } else {
                    local.applied_selection = node_state.selection.clone();
                }
            }
            Err(portal_error) => {
                health = "degraded".into();
                error = portal_error.to_string();
                if let Err(selection_error) = self.apply_selection(&local.applied_selection).await {
                    tracing::warn!(%selection_error, "cannot re-apply cached selection");
                }
            }
        }

        local.last_error = error.clone();
        local.updated_at = unix_now();
        state::save(&self.config.local_state_path(), &local).await?;

        let report = Report {
            revision: local.applied_revision.clone(),
            selection: local.applied_selection.clone(),
            health: health.clone(),
            error: error.clone(),
        };
        if let Err(report_error) = self.portal.report(&report).await {
            tracing::warn!(%report_error, "cannot report status to portal");
        }

        Ok(Outcome {
            revision: local.applied_revision,
            selection: local.applied_selection,
            health,
            error,
            applied,
        })
    }

    async fn apply_selection(&self, desired: &str) -> Result<String> {
        let tags = cached_tags(&self.config).await;
        let desired = if desired.is_empty() { AUTO_TAG } else { desired };
        apply::apply_selection(&self.clash, desired, &tags).await
    }

    pub async fn run(self) -> Result<()> {
        if self.config.once {
            return match self.run_once().await {
                Ok(outcome) => {
                    tracing::info!(
                        revision = %outcome.revision,
                        selection = %outcome.selection,
                        health = %outcome.health,
                        applied = outcome.applied,
                        "sync finished"
                    );
                    Ok(())
                }
                Err(error) if self.config.best_effort => {
                    tracing::warn!(%error, "sync failed, ignoring because of --best-effort");
                    Ok(())
                }
                Err(error) => Err(error),
            };
        }

        loop {
            if let Err(error) = self.run_once().await {
                tracing::warn!(%error, "sync failed");
            }
            tokio::time::sleep(self.config.interval).await;
        }
    }
}

async fn cached_tags(config: &AgentConfig) -> Vec<String> {
    match tokio::fs::read(config.outbounds_path()).await {
        Ok(bytes) => match serde_json::from_slice::<Vec<serde_json::Value>>(&bytes) {
            Ok(outbounds) => apply::tags_of(&outbounds),
            Err(error) => {
                tracing::warn!(%error, "cannot parse cached outbounds");
                Vec::new()
            }
        },
        Err(_) => Vec::new(),
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}
