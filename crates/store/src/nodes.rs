use sqlx::Row;

use crate::{NodeReport, Result, Selection, Store, now};

impl Store {
    pub async fn selections(&self) -> Result<Vec<Selection>> {
        let rows = sqlx::query("SELECT node_name, outbound, updated_at FROM selections ORDER BY node_name")
            .fetch_all(self.pool())
            .await?;

        rows.iter()
            .map(|row| {
                Ok(Selection {
                    node_name: row.try_get("node_name")?,
                    outbound: row.try_get("outbound")?,
                    updated_at: row.try_get("updated_at")?,
                })
            })
            .collect()
    }

    pub async fn selection(&self, node_name: &str) -> Result<Option<String>> {
        Ok(sqlx::query_scalar::<_, String>(
            "SELECT outbound FROM selections WHERE node_name = ?",
        )
        .bind(node_name)
        .fetch_optional(self.pool())
        .await?)
    }

    pub async fn set_selection(&self, node_name: &str, outbound: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO selections (node_name, outbound, updated_at) VALUES (?, ?, ?)
             ON CONFLICT(node_name) DO UPDATE SET outbound = excluded.outbound, updated_at = excluded.updated_at",
        )
        .bind(node_name)
        .bind(outbound)
        .bind(now())
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub async fn seed_selection(&self, node_name: &str, outbound: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO selections (node_name, outbound, updated_at) VALUES (?, ?, ?)
             ON CONFLICT(node_name) DO NOTHING",
        )
        .bind(node_name)
        .bind(outbound)
        .bind(now())
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub async fn reports(&self) -> Result<Vec<NodeReport>> {
        let rows = sqlx::query(
            "SELECT node_name, revision, selection, health, error, reported_at FROM node_reports ORDER BY node_name",
        )
        .fetch_all(self.pool())
        .await?;

        rows.iter().map(report_from_row).collect()
    }

    pub async fn report(&self, node_name: &str) -> Result<Option<NodeReport>> {
        let row = sqlx::query(
            "SELECT node_name, revision, selection, health, error, reported_at FROM node_reports WHERE node_name = ?",
        )
        .bind(node_name)
        .fetch_optional(self.pool())
        .await?;

        row.as_ref().map(report_from_row).transpose()
    }

    pub async fn record_report(&self, report: &NodeReport) -> Result<()> {
        sqlx::query(
            "INSERT INTO node_reports (node_name, revision, selection, health, error, reported_at)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(node_name) DO UPDATE SET revision = excluded.revision,
             selection = excluded.selection, health = excluded.health, error = excluded.error,
             reported_at = excluded.reported_at",
        )
        .bind(&report.node_name)
        .bind(&report.revision)
        .bind(&report.selection)
        .bind(&report.health)
        .bind(&report.error)
        .bind(report.reported_at)
        .execute(self.pool())
        .await?;
        Ok(())
    }
}

fn report_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<NodeReport> {
    Ok(NodeReport {
        node_name: row.try_get("node_name")?,
        revision: row.try_get("revision")?,
        selection: row.try_get("selection")?,
        health: row.try_get("health")?,
        error: row.try_get("error")?,
        reported_at: row.try_get("reported_at")?,
    })
}
