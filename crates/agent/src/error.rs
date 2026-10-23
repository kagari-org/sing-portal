#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("portal: {0}")]
    Portal(String),
    #[error("sing-box api: {0}")]
    BoxApi(String),
    #[error("sing-box check failed: {0}")]
    Check(String),
    #[error("sing-box did not become healthy within timeout")]
    Unhealthy,
    #[error("template: {0}")]
    Template(String),
    #[error("measurement failed: {0}")]
    Measure(String),
}

impl From<sing_portal_core::Error> for AgentError {
    fn from(error: sing_portal_core::Error) -> Self {
        AgentError::Template(error.to_string())
    }
}

pub type Result<T, E = AgentError> = std::result::Result<T, E>;
