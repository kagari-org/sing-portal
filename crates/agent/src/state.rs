use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Result;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalState {
    pub applied_revision: String,
    pub applied_selection: String,
    #[serde(default)]
    pub applied_latency_url: String,
    #[serde(default)]
    pub applied_renderer_version: u32,
    pub last_error: String,
    pub updated_at: i64,
}

pub async fn load(path: &Path) -> LocalState {
    match tokio::fs::read(path).await {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => LocalState::default(),
    }
}

pub async fn save(path: &Path, state: &LocalState) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(state)?;
    write_atomic(path, &bytes).await
}

pub async fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        tokio::fs::create_dir_all(parent).await?;
    }

    let temporary = path.with_extension("tmp");
    {
        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::File::create(&temporary).await?;
        file.write_all(bytes).await?;
        file.sync_all().await?;
    }
    tokio::fs::rename(&temporary, path).await?;
    Ok(())
}
