use crate::state::LocalState;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct HostError(pub String);

impl HostError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl From<std::io::Error> for HostError {
    fn from(error: std::io::Error) -> Self {
        Self(error.to_string())
    }
}

impl From<serde_json::Error> for HostError {
    fn from(error: serde_json::Error) -> Self {
        Self(error.to_string())
    }
}

pub type HostResult<T> = std::result::Result<T, HostError>;

/// Everything the agent cannot do portably. The agent decides *what* to apply;
/// the host decides where it lands and how sing-box picks it up.
///
/// Calls happen on the agent's runtime, so an implementation that blocks (a
/// restart command, a service reload) should still be fast or return early.
pub trait Host: Send + Sync {
    fn write_config(&self, config: &[u8]) -> HostResult<()>;

    /// Whether the config written last is still there; a fresh unit re-applies.
    fn has_config(&self) -> bool;

    fn restart(&self) -> HostResult<()>;

    fn load_state(&self) -> Option<LocalState>;

    fn save_state(&self, state: &LocalState) -> HostResult<()>;
}
