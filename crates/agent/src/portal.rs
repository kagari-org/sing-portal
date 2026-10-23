use std::time::Duration;

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::error::{AgentError, Result};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeState {
    pub revision: String,
    pub selection_revision: String,
    pub selection: String,
    pub outbounds: Vec<Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub revision: String,
    pub selection: String,
    pub health: String,
    pub error: String,
}

pub struct PortalClient {
    http: reqwest::Client,
    base: Url,
    node: String,
}

impl PortalClient {
    pub fn new(base: &str, node: &str) -> Result<Self> {
        let mut base =
            Url::parse(base).map_err(|error| AgentError::Portal(error.to_string()))?;
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|error| AgentError::Portal(error.to_string()))?;
        Ok(Self {
            http,
            base,
            node: node.to_string(),
        })
    }

    pub async fn fetch_state(
        &self,
        revision: Option<&str>,
        selection_revision: Option<&str>,
    ) -> Result<Option<NodeState>> {
        let mut url = self
            .base
            .join(&format!("api/v1/nodes/{}/state", self.node))
            .map_err(|error| AgentError::Portal(error.to_string()))?;

        {
            let mut query = url.query_pairs_mut();
            if let Some(revision) = revision.filter(|revision| !revision.is_empty()) {
                query.append_pair("revision", revision);
            }
            if let Some(selection_revision) =
                selection_revision.filter(|revision| !revision.is_empty())
            {
                query.append_pair("selectionRevision", selection_revision);
            }
        }

        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|error| AgentError::Portal(error.to_string()))?;

        match response.status() {
            StatusCode::NOT_MODIFIED => Ok(None),
            status if status.is_success() => {
                let body = response
                    .text()
                    .await
                    .map_err(|error| AgentError::Portal(error.to_string()))?;
                Ok(Some(serde_json::from_str(&body)?))
            }
            status => Err(AgentError::Portal(format!("state request returned {status}"))),
        }
    }

    pub async fn report(&self, report: &Report) -> Result<()> {
        let url = self
            .base
            .join(&format!("api/v1/nodes/{}/status", self.node))
            .map_err(|error| AgentError::Portal(error.to_string()))?;

        let response = self
            .http
            .post(url)
            .header("x-portal-request", "1")
            .json(report)
            .send()
            .await
            .map_err(|error| AgentError::Portal(error.to_string()))?;

        if !response.status().is_success() {
            return Err(AgentError::Portal(format!(
                "status report returned {}",
                response.status()
            )));
        }
        Ok(())
    }
}
