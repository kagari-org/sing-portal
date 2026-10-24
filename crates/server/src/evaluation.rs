use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluationProgress {
    pub machine: String,
    pub run_id: u64,
    pub running: bool,
    pub phase: String,
    pub done: usize,
    pub total: usize,
    pub current: Option<String>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub best: Option<String>,
    pub score: Option<f64>,
    pub switched: bool,
    pub checked: usize,
    pub error: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ServerEvent {
    Changed { scopes: Vec<String> },
    Evaluation { evaluation: EvaluationProgress },
}
