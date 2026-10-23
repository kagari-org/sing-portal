use serde_json::Value;

use crate::error::Error;
use crate::model::is_proxy_outbound;
use crate::uri;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Format {
    SingBoxJson,
    ClashYaml,
    Uri,
    UriBase64,
}

impl Format {
    pub fn as_str(self) -> &'static str {
        match self {
            Format::SingBoxJson => "sing-box-json",
            Format::ClashYaml => "clash-yaml",
            Format::Uri => "uri",
            Format::UriBase64 => "uri-base64",
        }
    }
}

impl std::str::FromStr for Format {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "sing-box-json" => Ok(Format::SingBoxJson),
            "clash-yaml" => Ok(Format::ClashYaml),
            "uri" => Ok(Format::Uri),
            "uri-base64" => Ok(Format::UriBase64),
            other => Err(Error::Uri(format!("unknown subscription format: {other}"))),
        }
    }
}

#[derive(Debug)]
pub struct ParsedSubscription {
    pub format: Format,
    pub outbounds: Vec<Value>,
    pub warnings: Vec<String>,
}

const KNOWN_SCHEMES: &[&str] = &[
    "vmess://",
    "vless://",
    "ss://",
    "trojan://",
    "hysteria2://",
    "hy2://",
    "tuic://",
    "anytls://",
    "socks://",
    "socks5://",
    "http://",
    "https://",
];

pub fn detect(content: &[u8]) -> Result<Format, Error> {
    let text = std::str::from_utf8(content).map_err(|_| Error::UnrecognizedFormat)?;
    let trimmed = text.trim_start_matches('\u{feff}').trim();

    if trimmed.is_empty() {
        return Err(Error::EmptyContent);
    }
    if trimmed.starts_with('<') {
        return Err(Error::HtmlContent);
    }

    if let Ok(json) = serde_json::from_str::<Value>(trimmed) {
        if json.get("outbounds").is_some_and(Value::is_array) {
            return Ok(Format::SingBoxJson);
        }
        if json.get("proxies").is_some() {
            return Ok(Format::ClashYaml);
        }
        if json
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item.is_object()))
        {
            return Ok(Format::SingBoxJson);
        }
    }

    for line in trimmed.lines() {
        let line = line.trim_start();
        if line.starts_with("proxies:") || line.starts_with("proxy-groups:") {
            return Ok(Format::ClashYaml);
        }
    }

    let decoded = uri::decode_base64(trimmed);
    if let Some(decoded) = decoded
        && decoded.contains("://")
    {
        return Ok(Format::UriBase64);
    }

    if KNOWN_SCHEMES.iter().any(|scheme| trimmed.contains(scheme)) {
        return Ok(Format::Uri);
    }

    Err(Error::UnrecognizedFormat)
}

pub fn parse_subscription(content: &[u8]) -> Result<ParsedSubscription, Error> {
    let format = detect(content)?;
    let text = std::str::from_utf8(content).map_err(|_| Error::UnrecognizedFormat)?;
    let trimmed = text.trim_start_matches('\u{feff}').trim();

    let (outbounds, warnings) = match format {
        Format::SingBoxJson => (parse_sing_box_json(trimmed)?, Vec::new()),
        Format::ClashYaml => crate::clash_yaml::parse(trimmed)?,
        Format::Uri => uri::parse_lines(trimmed)?,
        Format::UriBase64 => {
            let decoded = uri::decode_base64(trimmed)
                .ok_or_else(|| Error::Uri("subscription is not valid base64".into()))?;
            uri::parse_lines(&decoded)?
        }
    };

    if !outbounds.iter().any(is_proxy_outbound) {
        return Err(Error::NoOutbounds);
    }

    Ok(ParsedSubscription {
        format,
        outbounds,
        warnings,
    })
}

fn parse_sing_box_json(text: &str) -> Result<Vec<Value>, Error> {
    let json: Value = serde_json::from_str(text)?;
    let items = match &json {
        Value::Object(map) => map
            .get("outbounds")
            .and_then(Value::as_array)
            .cloned()
            .ok_or(Error::UnrecognizedFormat)?,
        Value::Array(items) => items.clone(),
        _ => return Err(Error::UnrecognizedFormat),
    };

    Ok(items
        .into_iter()
        .filter(|item| item.is_object())
        .collect())
}
