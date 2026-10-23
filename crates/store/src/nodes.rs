use sqlx::Row;

use crate::{Measurement, NodeReport, Result, Selection, SelectionMode, Store, StoreError, now};

impl Store {
    pub async fn selections(&self) -> Result<Vec<Selection>> {
        let rows = sqlx::query(
            "SELECT node_name, outbound, mode, updated_at FROM selections ORDER BY node_name",
        )
        .fetch_all(self.pool())
        .await?;

        rows.iter().map(selection_from_row).collect()
    }

    pub async fn selection(&self, node_name: &str) -> Result<Option<String>> {
        Ok(sqlx::query_scalar::<_, String>(
            "SELECT outbound FROM selections WHERE node_name = ?",
        )
        .bind(node_name)
        .fetch_optional(self.pool())
        .await?)
    }

    pub async fn selection_with_mode(&self, node_name: &str) -> Result<Option<Selection>> {
        let row = sqlx::query(
            "SELECT node_name, outbound, mode, updated_at FROM selections WHERE node_name = ?",
        )
        .bind(node_name)
        .fetch_optional(self.pool())
        .await?;

        row.as_ref().map(selection_from_row).transpose()
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

    pub async fn set_selection_mode(&self, node_name: &str, mode: SelectionMode) -> Result<()> {
        sqlx::query(
            "INSERT INTO selections (node_name, outbound, mode, updated_at) VALUES (?, 's_auto', ?, ?)
             ON CONFLICT(node_name) DO UPDATE SET mode = excluded.mode, updated_at = excluded.updated_at",
        )
        .bind(node_name)
        .bind(mode.as_str())
        .bind(now())
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub async fn seed_selection(&self, node_name: &str, outbound: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO selections (node_name, outbound, mode, updated_at) VALUES (?, ?, 'auto', ?)
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

    pub async fn replace_measurements(
        &self,
        node_name: &str,
        measurements: &[Measurement],
    ) -> Result<()> {
        let mut transaction = self.pool().begin().await?;
        sqlx::query("DELETE FROM measurements WHERE node_name = ?")
            .bind(node_name)
            .execute(&mut *transaction)
            .await?;

        for measurement in measurements {
            sqlx::query(
                "INSERT INTO measurements (node_name, tag, latency_ms, down_mbps, score, measured_at)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(&measurement.node_name)
            .bind(&measurement.tag)
            .bind(measurement.latency_ms)
            .bind(measurement.down_mbps)
            .bind(measurement.score)
            .bind(measurement.measured_at)
            .execute(&mut *transaction)
            .await?;
        }

        transaction.commit().await?;
        Ok(())
    }

    pub async fn measurements(&self, node_name: &str) -> Result<Vec<Measurement>> {
        let rows = sqlx::query(
            "SELECT node_name, tag, latency_ms, down_mbps, score, measured_at FROM measurements
             WHERE node_name = ? ORDER BY score DESC, down_mbps DESC, latency_ms IS NULL, latency_ms ASC, tag ASC",
        )
        .bind(node_name)
        .fetch_all(self.pool())
        .await?;

        rows.iter().map(measurement_from_row).collect()
    }
}

fn selection_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Selection> {
    let mode: String = row.try_get("mode")?;
    let mode = SelectionMode::parse(&mode)
        .ok_or_else(|| StoreError::Corrupt(format!("unknown selection mode {mode}")))?;

    Ok(Selection {
        node_name: row.try_get("node_name")?,
        outbound: row.try_get("outbound")?,
        mode,
        updated_at: row.try_get("updated_at")?,
    })
}

fn measurement_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Measurement> {
    Ok(Measurement {
        node_name: row.try_get("node_name")?,
        tag: row.try_get("tag")?,
        latency_ms: row.try_get("latency_ms")?,
        down_mbps: row.try_get("down_mbps")?,
        score: row.try_get("score")?,
        measured_at: row.try_get("measured_at")?,
    })
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
