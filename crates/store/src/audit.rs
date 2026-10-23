use sqlx::Row;

use crate::{AuditEntry, Result, Store};

impl Store {
    pub async fn record_audit(
        &self,
        actor: &str,
        action: &str,
        target: &str,
        detail: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO audit_log (at, actor, action, target, detail) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(crate::now())
        .bind(actor)
        .bind(action)
        .bind(target)
        .bind(detail)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub async fn audit_entries(&self, limit: i64) -> Result<Vec<AuditEntry>> {
        let rows = sqlx::query(
            "SELECT id, at, actor, action, target, detail FROM audit_log ORDER BY id DESC LIMIT ?",
        )
        .bind(limit.clamp(1, 1000))
        .fetch_all(self.pool())
        .await?;

        rows.iter()
            .map(|row| {
                Ok(AuditEntry {
                    id: row.try_get("id")?,
                    at: row.try_get("at")?,
                    actor: row.try_get("actor")?,
                    action: row.try_get("action")?,
                    target: row.try_get("target")?,
                    detail: row.try_get("detail")?,
                })
            })
            .collect()
    }
}
