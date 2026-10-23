use std::time::Duration;

use serde_json::{Value, json};
use url::Url;

use crate::error::Error;
use crate::render::SELECTOR_TAG;

#[derive(Clone)]
pub struct ClashClient {
    http: reqwest::Client,
    base: Url,
}

impl ClashClient {
    pub fn new(base: &str, timeout: Duration) -> Result<Self, Error> {
        let mut base =
            Url::parse(base).map_err(|err| Error::ClashApi(format!("invalid base url: {err}")))?;
        if !base.path().ends_with('/') {
            let path = format!("{}/", base.path());
            base.set_path(&path);
        }
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|err| Error::ClashApi(err.to_string()))?;
        Ok(Self { http, base })
    }

    fn url(&self, path: &str) -> Result<Url, Error> {
        self.base
            .join(path)
            .map_err(|err| Error::ClashApi(err.to_string()))
    }

    fn proxy_path(&self, name: &str, suffix: &str) -> Result<Url, Error> {
        let mut url = self.base.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|()| Error::ClashApi("base url cannot be a base".into()))?;
            segments.push("proxies").push(name);
            if let Some(suffix) = suffix.strip_prefix('/').filter(|suffix| !suffix.is_empty()) {
                segments.push(suffix);
            }
        }
        Ok(url)
    }

    pub async fn version(&self) -> Result<String, Error> {
        let response = self.http.get(self.url("version")?).send().await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(Error::ClashApi(format!("version returned {status}: {body}")));
        }
        let value: Value = serde_json::from_str(&body)?;
        Ok(value
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string())
    }

    pub async fn proxies(&self) -> Result<Value, Error> {
        let response = self.http.get(self.url("proxies")?).send().await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(Error::ClashApi(format!("proxies returned {status}")));
        }
        Ok(serde_json::from_str(&body)?)
    }

    pub async fn current_selection(&self) -> Result<String, Error> {
        let response = self
            .http
            .get(self.proxy_path(SELECTOR_TAG, "")?)
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(Error::ClashApi(format!(
                "proxy {SELECTOR_TAG} returned {status}"
            )));
        }
        let value: Value = serde_json::from_str(&body)?;
        Ok(value
            .get("now")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string())
    }

    pub async fn select(&self, group: &str, name: &str) -> Result<(), Error> {
        let response = self
            .http
            .put(self.proxy_path(group, "")?)
            .json(&json!({ "name": name }))
            .send()
            .await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::ClashApi(format!(
                "select {group} -> {name} returned {status}: {body}"
            )));
        }
        Ok(())
    }

    pub async fn delay(&self, name: &str, url: &str, timeout_ms: u64) -> Result<u64, Error> {
        let mut target = self.proxy_path(name, "/delay")?;
        target
            .query_pairs_mut()
            .append_pair("url", url)
            .append_pair("timeout", &timeout_ms.to_string());
        let response = self.http.get(target).send().await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(Error::ClashApi(format!(
                "delay {name} returned {status}: {body}"
            )));
        }
        let value: Value = serde_json::from_str(&body)?;
        value
            .get("delay")
            .and_then(Value::as_u64)
            .ok_or_else(|| Error::ClashApi("delay response has no delay field".into()))
    }
}
