use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use sing_portal_core::render::SELECTOR_TAG;
use tokio::sync::RwLock;
use tonic::Status;
use tonic::metadata::{Ascii, MetadataValue};
use tonic::service::Interceptor;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::{Channel, Endpoint};

use crate::error::{AgentError, Result};

pub mod daemon {
    tonic::include_proto!("daemon");
}

use daemon::started_service_client::StartedServiceClient;
use daemon::{SelectOutboundRequest, UrlTestRequest};

/// libbox rejects calls that do not carry the configured command secret, so the
/// header rides on every request instead of being set at the call sites.
#[derive(Clone)]
struct CommandSecretInterceptor {
    secret: Option<MetadataValue<Ascii>>,
}

impl Interceptor for CommandSecretInterceptor {
    fn call(
        &mut self,
        mut request: tonic::Request<()>,
    ) -> std::result::Result<tonic::Request<()>, Status> {
        if let Some(secret) = &self.secret {
            request
                .metadata_mut()
                .insert("x-command-secret", secret.clone());
        }
        Ok(request)
    }
}

type CommandClient = StartedServiceClient<InterceptedService<Channel, CommandSecretInterceptor>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutboundState {
    pub url_test_time: i64,
    pub delay_ms: Option<u32>,
}

#[derive(Clone)]
pub struct BoxApi {
    url: String,
    channel: Arc<RwLock<Channel>>,
    secret: Option<MetadataValue<Ascii>>,
    timeout: Duration,
}

impl BoxApi {
    pub fn new(url: &str, api_secret: Option<&str>, timeout: Duration) -> Result<Self> {
        let channel = Self::connect(url)?;
        let secret = api_secret
            .filter(|secret| !secret.is_empty())
            .map(MetadataValue::<Ascii>::try_from)
            .transpose()
            .map_err(|error| AgentError::BoxApi(format!("invalid api secret: {error}")))?;
        Ok(Self {
            url: url.to_string(),
            channel: Arc::new(RwLock::new(channel)),
            secret,
            timeout,
        })
    }

    fn connect(url: &str) -> Result<Channel> {
        let endpoint = Endpoint::from_shared(url.to_string())
            .map_err(|error| AgentError::BoxApi(format!("invalid api url: {error}")))?;
        Ok(endpoint.connect_lazy())
    }

    async fn client(&self) -> CommandClient {
        let channel = self.channel.read().await.clone();
        StartedServiceClient::with_interceptor(
            channel,
            CommandSecretInterceptor {
                secret: self.secret.clone(),
            },
        )
    }

    /// sing-box restarts drop every connection; a lazy channel that lost its
    /// transport keeps failing until tonic reconnects with backoff, so replace
    /// it after transport errors instead of waiting for that.
    async fn reconnect(&self) {
        if let Ok(channel) = Self::connect(&self.url) {
            *self.channel.write().await = channel;
        }
    }

    async fn wait<T>(
        &self,
        timeout: Duration,
        request: impl Future<Output = std::result::Result<T, Status>>,
    ) -> Result<T> {
        match tokio::time::timeout(timeout, request).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(status)) => {
                // a restarted sing-box can answer on a stale h2 connection with
                // a body-less error; any channel state is suspect after an error
                self.reconnect().await;
                Err(AgentError::BoxApi(status.message().to_string()))
            }
            Err(_) => {
                self.reconnect().await;
                Err(AgentError::BoxApi("sing-box api request timed out".into()))
            }
        }
    }

    pub async fn version(&self) -> Result<String> {
        self.version_within(self.timeout).await
    }

    /// Version probe with a custom timeout, so a reconnecting channel cannot
    /// eat the whole health check deadline in a single call.
    pub async fn version_within(&self, timeout: Duration) -> Result<String> {
        let mut client = self.client().await;
        let reply = self.wait(timeout, client.get_version(())).await?;
        Ok(reply.into_inner().version)
    }

    pub async fn current_selection(&self) -> Result<String> {
        let mut client = self.client().await;
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
        let mut client = self.client().await;
        let request = SelectOutboundRequest {
            group_tag: group.to_string(),
            outbound_tag: outbound.to_string(),
        };
        self.wait(self.timeout, client.select_outbound(request))
            .await?;
        Ok(())
    }

    pub async fn url_test(&self, outbound: &str) -> Result<()> {
        let mut client = self.client().await;
        let request = UrlTestRequest {
            outbound_tag: outbound.to_string(),
        };
        self.wait(self.timeout, client.url_test(request)).await?;
        Ok(())
    }

    pub async fn outbounds(&self) -> Result<HashMap<String, OutboundState>> {
        let mut client = self.client().await;
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use tonic::service::Interceptor;

    #[test]
    fn sends_command_secret_when_configured() {
        let mut interceptor = CommandSecretInterceptor {
            secret: Some(MetadataValue::from_static("abc")),
        };
        let request = interceptor.call(tonic::Request::new(())).unwrap();
        assert_eq!(
            request
                .metadata()
                .get("x-command-secret")
                .unwrap()
                .to_str()
                .unwrap(),
            "abc"
        );
    }

    #[test]
    fn omits_command_secret_when_not_configured() {
        let mut interceptor = CommandSecretInterceptor { secret: None };
        let request = interceptor.call(tonic::Request::new(())).unwrap();
        assert!(request.metadata().get("x-command-secret").is_none());
    }

    #[test]
    fn empty_secret_produces_no_interceptor_value() {
        let secret = Some("")
            .filter(|secret| !secret.is_empty())
            .map(MetadataValue::<Ascii>::try_from)
            .transpose()
            .unwrap();
        assert!(secret.is_none());
    }
}
