use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use sing_portal_core::render::SELECTOR_TAG;

use crate::error::{AgentError, Result};

#[derive(Clone)]
pub struct BoxApi {
    binary: PathBuf,
    url: String,
    timeout: Duration,
}

impl BoxApi {
    pub fn new(binary: impl Into<PathBuf>, url: impl Into<String>, timeout: Duration) -> Self {
        Self {
            binary: binary.into(),
            url: url.into(),
            timeout,
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    async fn run(&self, args: &[&str]) -> Result<String> {
        let output = tokio::time::timeout(
            self.timeout,
            tokio::process::Command::new(&self.binary)
                .arg("api")
                .args(args)
                .args(["--url", &self.url])
                .output(),
        )
        .await
        .map_err(|_| AgentError::BoxApi(format!("sing-box api {} timed out", args.join(" "))))?
        .map_err(|error| AgentError::BoxApi(format!("cannot run sing-box api: {error}")))?;

        if !output.status.success() {
            return Err(AgentError::BoxApi(format!(
                "sing-box api {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    pub async fn version(&self) -> Result<String> {
        Ok(self.run(&["version"]).await?.trim().to_string())
    }

    pub async fn current_selection(&self) -> Result<String> {
        let text = self.run(&["group", "show", SELECTOR_TAG]).await?;
        select_field(&text, "Selected:").ok_or_else(|| {
            AgentError::BoxApi(format!("group {SELECTOR_TAG} reports no selected outbound"))
        })
    }

    pub async fn select(&self, group: &str, outbound: &str) -> Result<()> {
        self.run(&["group", "select", group, outbound])
            .await
            .map(|_| ())
    }

    pub async fn url_test(&self, group: &str) -> Result<()> {
        self.run(&["group", "urltest", group]).await.map(|_| ())
    }

    pub async fn delays(&self) -> Result<HashMap<String, Option<u64>>> {
        Ok(parse_delays(&self.run(&["outbounds"]).await?))
    }

    pub async fn network_quality(
        &self,
        tag: &str,
        runtime: u64,
        config_url: Option<&str>,
    ) -> Result<String> {
        let runtime = runtime.to_string();
        let mut args = vec!["networkquality", "-o", tag, "--max-runtime", &runtime];
        if let Some(config_url) = config_url {
            args.push("--config-url");
            args.push(config_url);
        }
        self.run(&args).await
    }
}

fn select_field(text: &str, prefix: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let value = line.strip_prefix(prefix)?.trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

fn parse_delays(text: &str) -> HashMap<String, Option<u64>> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let tag = fields.next()?.trim();
            let _kind = fields.next()?;
            if tag.is_empty() {
                return None;
            }
            let delay = fields.next().unwrap_or("-").trim();
            let millis = delay
                .strip_suffix("ms")
                .and_then(|value| value.trim().parse().ok());
            Some((tag.to_string(), millis))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{parse_delays, select_field};

    #[test]
    fn parses_group_show_selection() {
        let text = "Tag:       s_out\nType:      selector\nSelected:  provider-a · JP 02\n";
        assert_eq!(
            select_field(text, "Selected:").as_deref(),
            Some("provider-a · JP 02")
        );
    }

    #[test]
    fn parses_outbound_delays() {
        let text = "s_out\tselector\t96 ms\ns_auto\turltest\t96 ms\nbad\tsocks\t-\n";
        let delays = parse_delays(text);
        assert_eq!(delays.get("s_out"), Some(&Some(96)));
        assert_eq!(delays.get("bad"), Some(&None));
    }
}
