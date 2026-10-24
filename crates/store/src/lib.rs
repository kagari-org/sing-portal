use std::path::Path;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Row, SqlitePool};

pub mod audit;
pub mod export;
pub mod model;
pub mod nodes;
pub mod subscriptions;

pub use model::{
    AuditEntry, ManualMeasurement, Measurement, NewSubscription, NodeReport, Selection,
    SelectionMode, Snapshot, Source, Subscription, SubscriptionPatch,
};

const SCHEMA: &str = include_str!("../../../migrations/0001_init.sql");

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("corrupt row: {0}")]
    Corrupt(String),
    #[error("not found")]
    NotFound,
}

pub type Result<T, E = StoreError> = std::result::Result<T, E>;

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    pub async fn connect(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }

        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));

        Self::connect_with(options).await
    }

    pub async fn connect_with(options: SqliteConnectOptions) -> Result<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await?;
        let store = Self { pool };
        store.migrate().await?;
        Ok(store)
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    async fn migrate(&self) -> Result<()> {
        sqlx::raw_sql(SCHEMA).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn healthz(&self) -> Result<()> {
        sqlx::query_scalar::<_, i64>("SELECT 1")
            .fetch_one(&self.pool)
            .await?;
        Ok(())
    }
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

pub(crate) const SUBSCRIPTION_COLUMNS: &str = "id, name, slug, source, url, user_agent, format, content, outbounds, enabled, auto_refresh, refresh_interval_secs, next_refresh_at, last_fetched_at, last_error, failures, created_at, updated_at";

pub(crate) fn subscription_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Subscription> {
    let outbounds: String = row.try_get("outbounds")?;
    let outbounds = serde_json::from_str(&outbounds)
        .map_err(|err| StoreError::Corrupt(format!("outbounds: {err}")))?;
    let source: String = row.try_get("source")?;
    let source = Source::parse(&source)
        .ok_or_else(|| StoreError::Corrupt(format!("unknown source {source}")))?;

    Ok(Subscription {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        slug: row.try_get("slug")?,
        source,
        url: row.try_get("url")?,
        user_agent: row.try_get("user_agent")?,
        format: row.try_get("format")?,
        content: row.try_get("content")?,
        outbounds,
        enabled: row.try_get::<i64, _>("enabled")? != 0,
        auto_refresh: row.try_get::<i64, _>("auto_refresh")? != 0,
        refresh_interval_secs: row.try_get("refresh_interval_secs")?,
        next_refresh_at: row.try_get("next_refresh_at")?,
        last_fetched_at: row.try_get("last_fetched_at")?,
        last_error: row.try_get("last_error")?,
        failures: row.try_get("failures")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

pub(crate) fn bind_outbounds(outbounds: &[serde_json::Value]) -> Result<String> {
    serde_json::to_string(outbounds).map_err(|err| StoreError::Corrupt(format!("outbounds: {err}")))
}
