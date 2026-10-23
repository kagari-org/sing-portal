use crate::{Result, Snapshot, Store, StoreError, bind_outbounds, now};

impl Store {
    pub async fn snapshot(&self) -> Result<Snapshot> {
        Ok(Snapshot {
            version: 1,
            exported_at: now(),
            subscriptions: self.list_subscriptions().await?,
            selections: self.selections().await?,
        })
    }

    pub async fn import_snapshot(&self, snapshot: &Snapshot) -> Result<()> {
        if snapshot.version != 1 {
            return Err(StoreError::Corrupt(format!(
                "unsupported snapshot version {}",
                snapshot.version
            )));
        }

        let mut transaction = self.pool().begin().await?;
        sqlx::query("DELETE FROM subscriptions")
            .execute(&mut *transaction)
            .await?;

        for subscription in &snapshot.subscriptions {
            sqlx::query(
                "INSERT INTO subscriptions (
                    id, name, slug, source, url, user_agent, format, content, outbounds, enabled,
                    auto_refresh, refresh_interval_secs, next_refresh_at, last_fetched_at,
                    last_error, validated, failures, created_at, updated_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&subscription.id)
            .bind(&subscription.name)
            .bind(&subscription.slug)
            .bind(subscription.source.as_str())
            .bind(&subscription.url)
            .bind(&subscription.user_agent)
            .bind(&subscription.format)
            .bind(&subscription.content)
            .bind(bind_outbounds(&subscription.outbounds)?)
            .bind(i64::from(subscription.enabled))
            .bind(i64::from(subscription.auto_refresh))
            .bind(subscription.refresh_interval_secs)
            .bind(subscription.next_refresh_at)
            .bind(subscription.last_fetched_at)
            .bind(&subscription.last_error)
            .bind(i64::from(subscription.validated))
            .bind(subscription.failures)
            .bind(subscription.created_at)
            .bind(subscription.updated_at)
            .execute(&mut *transaction)
            .await?;
        }

        sqlx::query("DELETE FROM selections")
            .execute(&mut *transaction)
            .await?;
        for selection in &snapshot.selections {
            sqlx::query(
                "INSERT INTO selections (node_name, outbound, mode, updated_at) VALUES (?, ?, ?, ?)",
            )
            .bind(&selection.node_name)
            .bind(&selection.outbound)
            .bind(selection.mode.as_str())
            .bind(selection.updated_at)
            .execute(&mut *transaction)
            .await?;
        }

        transaction.commit().await?;
        Ok(())
    }
}
