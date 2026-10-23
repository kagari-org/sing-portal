use std::collections::HashMap;
use std::future::Future;
use std::time::{Duration, Instant};

use sing_portal_core::render::SELECTOR_TAG;
use tonic::Status;
use tonic::transport::{Channel, Endpoint};

use crate::error::{AgentError, Result};

pub mod daemon {
    tonic::include_proto!("daemon");
}

use daemon::started_service_client::StartedServiceClient;
use daemon::{
    NetworkQualityTestRequest, NetworkQualityTestProgress, SelectOutboundRequest, UrlTestRequest,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutboundState {
    pub url_test_time: i64,
    pub delay_ms: Option<u32>,
}

#[derive(Clone)]
pub struct BoxApi {
    channel: Channel,
    timeout: Duration,
}

impl BoxApi {
    pub fn new(url: &str, timeout: Duration) -> Result<Self> {
        let endpoint = Endpoint::from_shared(url.to_string())
            .map_err(|error| AgentError::BoxApi(format!("invalid api url: {error}")))?;
        Ok(Self {
            channel: endpoint.connect_lazy(),
            timeout,
        })
    }

    fn client(&self) -> StartedServiceClient<Channel> {
        StartedServiceClient::new(self.channel.clone())
    }

    async fn wait<T>(
        &self,
        timeout: Duration,
        request: impl Future<Output = std::result::Result<T, Status>>,
    ) -> Result<T> {
        tokio::time::timeout(timeout, request)
            .await
            .map_err(|_| AgentError::BoxApi("sing-box api request timed out".into()))?
            .map_err(|status| AgentError::BoxApi(status.message().to_string()))
    }

    pub async fn version(&self) -> Result<String> {
        let mut client = self.client();
        let reply = self.wait(self.timeout, client.get_version(())).await?;
        Ok(reply.into_inner().version)
    }

    pub async fn current_selection(&self) -> Result<String> {
        let mut client = self.client();
        let mut stream = self
            .wait(self.timeout, client.subscribe_groups(()))
            .await?
            .into_inner();
        loop {
            match self.wait(self.timeout, stream.message()).await? {
                Some(snapshot) => {
                    if let Some(group) = snapshot
                        .group
                        .iter()
                        .find(|group| group.tag == SELECTOR_TAG)
                    {
                        return Ok(group.selected.clone());
                    }
                }
                None => {
                    return Err(AgentError::BoxApi(
                        "sing-box closed the groups stream".into(),
                    ));
                }
            }
        }
    }

    pub async fn select(&self, group: &str, outbound: &str) -> Result<()> {
        let mut client = self.client();
        let request = SelectOutboundRequest {
            group_tag: group.to_string(),
            outbound_tag: outbound.to_string(),
        };
        self.wait(self.timeout, client.select_outbound(request))
            .await?;
        Ok(())
    }

    pub async fn url_test(&self, outbound: &str) -> Result<()> {
        let mut client = self.client();
        let request = UrlTestRequest {
            outbound_tag: outbound.to_string(),
        };
        self.wait(self.timeout, client.url_test(request)).await?;
        Ok(())
    }

    pub async fn outbounds(&self) -> Result<HashMap<String, OutboundState>> {
        let mut client = self.client();
        let mut stream = self
            .wait(self.timeout, client.subscribe_outbounds(()))
            .await?
            .into_inner();
        let first = self
            .wait(self.timeout, stream.message())
            .await?
            .ok_or_else(|| AgentError::BoxApi("sing-box closed the outbounds stream".into()))?;

        Ok(first
            .outbounds
            .into_iter()
            .filter(|item| !item.tag.is_empty())
            .map(|item| {
                (
                    item.tag,
                    OutboundState {
                        url_test_time: item.url_test_time,
                        delay_ms: (item.url_test_delay > 0).then_some(item.url_test_delay as u32),
                    },
                )
            })
            .collect())
    }

    pub async fn network_quality(
        &self,
        tag: &str,
        runtime: u64,
        config_url: Option<&str>,
    ) -> Result<NetworkQualityTestProgress> {
        let mut client = self.client();
        let request = NetworkQualityTestRequest {
            config_url: config_url.unwrap_or_default().to_string(),
            outbound_tag: tag.to_string(),
            serial: false,
            max_runtime_seconds: runtime as i32,
            http3: false,
        };
        let mut stream = self
            .wait(self.timeout, client.start_network_quality_test(request))
            .await?
            .into_inner();

        let deadline = Instant::now() + Duration::from_secs(runtime.max(1) + 30);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(AgentError::BoxApi(
                    "network quality test did not finish in time".into(),
                ));
            }
            let progress = self
                .wait(remaining, stream.message())
                .await?
                .ok_or_else(|| {
                    AgentError::BoxApi("sing-box closed the quality stream".into())
                })?;
            if !progress.error.is_empty() {
                return Err(AgentError::BoxApi(progress.error));
            }
            if progress.is_final {
                return Ok(progress);
            }
        }
    }
}
