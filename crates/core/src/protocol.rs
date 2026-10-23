use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ServerMessage {
    Sync {
        id: String,
        revision: String,
        selection_revision: String,
        selection: String,
        outbounds: Vec<Value>,
    },
    Test {
        id: String,
        kind: String,
        tag: String,
        url: Option<String>,
    },
    Select {
        id: String,
        outbound: String,
    },
    Proxies {
        id: String,
    },
}

impl ServerMessage {
    pub fn id(&self) -> &str {
        match self {
            ServerMessage::Sync { id, .. }
            | ServerMessage::Test { id, .. }
            | ServerMessage::Select { id, .. }
            | ServerMessage::Proxies { id } => id,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AgentMessage {
    Hello {
        name: String,
        version: String,
    },
    Status {
        revision: String,
        selection: String,
        health: String,
        error: String,
    },
    SyncResult {
        id: String,
        revision: String,
        selection: String,
        health: String,
        error: String,
        applied: bool,
    },
    TestResult {
        id: String,
        latency_ms: Option<f64>,
        bytes: Option<u64>,
        seconds: Option<f64>,
        down_mbps: Option<f64>,
        restored: String,
    },
    SelectResult {
        id: String,
        applied: bool,
    },
    ProxiesResult {
        id: String,
        proxies: Value,
    },
    Error {
        id: Option<String>,
        message: String,
    },
}

impl AgentMessage {
    pub fn id(&self) -> Option<&str> {
        match self {
            AgentMessage::Hello { .. } | AgentMessage::Status { .. } => None,
            AgentMessage::SyncResult { id, .. }
            | AgentMessage::TestResult { id, .. }
            | AgentMessage::SelectResult { id, .. }
            | AgentMessage::ProxiesResult { id, .. } => Some(id),
            AgentMessage::Error { id, .. } => id.as_deref(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sync_round_trip() {
        let message = ServerMessage::Sync {
            id: "1".into(),
            revision: "sha256:abc".into(),
            selection_revision: "sha256:def".into(),
            selection: "sub · node".into(),
            outbounds: vec![json!({"type": "direct", "tag": "sub · node"})],
        };
        let encoded = serde_json::to_string(&message).unwrap();
        assert!(encoded.contains("\"type\":\"sync\""));
        let decoded: ServerMessage = serde_json::from_str(&encoded).unwrap();
        assert_eq!(message, decoded);
    }

    #[test]
    fn agent_messages_are_tagged() {
        let status = AgentMessage::Status {
            revision: "none".into(),
            selection: "s_auto".into(),
            health: "ok".into(),
            error: String::new(),
        };
        assert_eq!(status.id(), None);

        let result = AgentMessage::TestResult {
            id: "42".into(),
            latency_ms: Some(12.5),
            bytes: Some(1024),
            seconds: Some(0.5),
            down_mbps: Some(16.0),
            restored: "s_auto".into(),
        };
        assert_eq!(result.id(), Some("42"));

        let encoded = serde_json::to_string(&result).unwrap();
        assert!(encoded.contains("\"type\":\"testResult\""));
    }
}
