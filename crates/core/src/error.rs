#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("empty subscription content")]
    EmptyContent,
    #[error("subscription content looks like an HTML page; the link is probably expired or requires a login")]
    HtmlContent,
    #[error("unrecognized subscription format")]
    UnrecognizedFormat,
    #[error("subscription contains {0} outbounds, limit is {1}")]
    TooManyOutbounds(usize, usize),
    #[error("no usable outbound found in subscription")]
    NoOutbounds,
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid YAML: {0}")]
    Yaml(String),
    #[error("invalid URI: {0}")]
    Uri(String),
    #[error("unsupported URI scheme: {0}")]
    UnsupportedScheme(String),
    #[error("template must be a JSON object")]
    TemplateNotObject,
    #[error("template must not contain an outbounds key; the renderer owns it")]
    TemplateHasOutbounds,
}
