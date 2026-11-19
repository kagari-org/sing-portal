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
    /// The portal URL the current config was rendered for.
    #[serde(default)]
    pub applied_portal_url: String,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_state_defaults_applied_portal_url() {
        let state: LocalState = serde_json::from_str(r#"{"appliedRevision":"sha256:x"}"#).unwrap();
        assert!(state.applied_portal_url.is_empty());
    }

    #[test]
    fn local_state_round_trips_applied_portal_url() {
        let state = LocalState {
            applied_portal_url: "wss://portal.example.com".into(),
            ..LocalState::default()
        };
        let encoded = serde_json::to_string(&state).unwrap();
        assert!(encoded.contains("\"appliedPortalUrl\":\"wss://portal.example.com\""));
        let decoded: LocalState = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.applied_portal_url, "wss://portal.example.com");
    }
}
