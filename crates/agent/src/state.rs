use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalState {
    #[serde(default)]
    pub applied_revision: String,
    #[serde(default)]
    pub applied_selection: String,
    #[serde(default)]
    pub applied_latency_url: String,
    #[serde(default)]
    pub applied_renderer_version: u32,
    #[serde(default)]
    pub applied_template_hash: String,
    /// The last config that came up healthy, kept for the next rollback.
    #[serde(default)]
    pub previous_config: Option<String>,
    #[serde(default)]
    pub last_error: String,
    #[serde(default)]
    pub updated_at: i64,
}
