use std::path::{Path, PathBuf};

use sing_portal_agent::state::LocalState;
use sing_portal_agent::{Host, HostError, HostResult};

/// Desktop side effects: a file sing-box reads on start, a command that
/// restarts it, and a JSON file for the agent's own state.
pub struct DesktopHost {
    config_path: PathBuf,
    state_path: PathBuf,
    restart_cmd: Option<String>,
}

impl DesktopHost {
    pub fn new(config_path: PathBuf, state_dir: PathBuf, restart_cmd: Option<String>) -> Self {
        Self {
            config_path,
            state_path: state_dir.join("state.json"),
            restart_cmd,
        }
    }
}

impl Host for DesktopHost {
    fn write_config(&self, config: &[u8]) -> HostResult<()> {
        write_atomic(&self.config_path, config)
    }

    fn has_config(&self) -> bool {
        self.config_path.exists()
    }

    fn restart(&self) -> HostResult<()> {
        let Some(command) = self
            .restart_cmd
            .as_deref()
            .filter(|command| !command.is_empty())
        else {
            return Ok(());
        };

        let output = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(command)
            .output()?;

        if !output.status.success() {
            return Err(HostError::new(format!(
                "restart command failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(())
    }

    fn load_state(&self) -> Option<LocalState> {
        let bytes = std::fs::read(&self.state_path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn save_state(&self, state: &LocalState) -> HostResult<()> {
        let bytes = serde_json::to_vec_pretty(state)?;
        write_atomic(&self.state_path, &bytes)
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> HostResult<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }

    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}
