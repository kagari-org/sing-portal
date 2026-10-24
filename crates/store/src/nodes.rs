use std::collections::HashSet;

use sqlx::Row;

use crate::{
    ManualMeasurement, Measurement, NodeReport, Result, Selection, SelectionMode, Store,
    StoreError, now,
};

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
        Ok(
            sqlx::query_scalar::<_, String>("SELECT outbound FROM selections WHERE node_name = ?")
                .bind(node_name)
                .fetch_optional(self.pool())
                .await?,
        )
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

    pub async fn clear_measurements(&self, node_name: &str) -> Result<()> {
        sqlx::query("DELETE FROM measurements WHERE node_name = ?")
            .bind(node_name)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub async fn upsert_measurement(&self, measurement: &Measurement) -> Result<()> {
        sqlx::query(
            "INSERT INTO measurements (node_name, tag, latency_ms, down_mbps, score, measured_at)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(node_name, tag) DO UPDATE SET
                latency_ms = excluded.latency_ms,
                down_mbps = excluded.down_mbps,
                score = excluded.score,
                measured_at = excluded.measured_at",
        )
        .bind(&measurement.node_name)
        .bind(&measurement.tag)
        .bind(measurement.latency_ms)
        .bind(measurement.down_mbps)
        .bind(measurement.score)
        .bind(measurement.measured_at)
        .execute(self.pool())
        .await?;
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

    /// Manual measurements survive automatic evaluations; a latency-only test
    /// keeps the previous bandwidth (and the other way around).
    pub async fn record_manual_measurement(
        &self,
        node_name: &str,
        tag: &str,
        latency_ms: Option<f64>,
        down_mbps: Option<f64>,
        measured_at: i64,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO manual_measurements (node_name, tag, latency_ms, down_mbps, measured_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(node_name, tag) DO UPDATE SET
                latency_ms = COALESCE(excluded.latency_ms, manual_measurements.latency_ms),
                down_mbps = COALESCE(excluded.down_mbps, manual_measurements.down_mbps),
                measured_at = excluded.measured_at",
        )
        .bind(node_name)
        .bind(tag)
        .bind(latency_ms)
        .bind(down_mbps)
        .bind(measured_at)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub async fn manual_measurements(&self, node_name: &str) -> Result<Vec<ManualMeasurement>> {
        let rows = sqlx::query(
            "SELECT node_name, tag, latency_ms, down_mbps, measured_at FROM manual_measurements
             WHERE node_name = ? ORDER BY measured_at DESC, tag ASC",
        )
        .bind(node_name)
        .fetch_all(self.pool())
        .await?;

        rows.iter().map(manual_measurement_from_row).collect()
    }

    pub async fn disabled_tags(&self) -> Result<HashSet<String>> {
        let tags = sqlx::query_scalar::<_, String>("SELECT tag FROM disabled_nodes")
            .fetch_all(self.pool())
            .await?;
        Ok(tags.into_iter().collect())
    }

    pub async fn set_node_disabled(&self, tag: &str, disabled: bool) -> Result<()> {
        if disabled {
            sqlx::query(
                "INSERT INTO disabled_nodes (tag, disabled_at) VALUES (?, ?)
                 ON CONFLICT(tag) DO UPDATE SET disabled_at = excluded.disabled_at",
            )
            .bind(tag)
            .bind(now())
            .execute(self.pool())
            .await?;
        } else {
            sqlx::query("DELETE FROM disabled_nodes WHERE tag = ?")
                .bind(tag)
                .execute(self.pool())
                .await?;
        }
        Ok(())
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

fn manual_measurement_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<ManualMeasurement> {
    Ok(ManualMeasurement {
        node_name: row.try_get("node_name")?,
        tag: row.try_get("tag")?,
        latency_ms: row.try_get("latency_ms")?,
        down_mbps: row.try_get("down_mbps")?,
        measured_at: row.try_get("measured_at")?,
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
