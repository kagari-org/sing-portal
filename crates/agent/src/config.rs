use std::path::PathBuf;
use std::time::Duration;

use crate::tester::TesterConfig;

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub portal_url: String,
    pub node_name: String,
    pub template_path: PathBuf,
    pub config_path: PathBuf,
    pub state_dir: PathBuf,
    pub sing_box_bin: String,
    pub clash_url: String,
    pub restart_cmd: Option<String>,
    pub interval: Duration,
    pub once: bool,
    pub best_effort: bool,
    pub health_timeout: Duration,
    pub tester: TesterConfig,
}

impl AgentConfig {
    pub fn outbounds_path(&self) -> PathBuf {
        self.state_dir.join("outbounds.json")
    }

    pub fn outbounds_previous_path(&self) -> PathBuf {
        self.state_dir.join("outbounds.prev.json")
    }

    pub fn local_state_path(&self) -> PathBuf {
        self.state_dir.join("state.json")
    }

    pub fn config_next_path(&self) -> PathBuf {
        self.config_path
            .with_file_name(format!(
                "{}.next",
                self.config_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("config.json")
            ))
    }
}
