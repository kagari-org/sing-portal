use crate::{
    NewSubscription, Result, SUBSCRIPTION_COLUMNS, Store, StoreError, Subscription,
    SubscriptionPatch, bind_outbounds, now, subscription_from_row,
};

impl Store {
    pub async fn list_subscriptions(&self) -> Result<Vec<Subscription>> {
        let rows = sqlx::query(&format!(
            "SELECT {SUBSCRIPTION_COLUMNS} FROM subscriptions ORDER BY created_at ASC, id ASC"
        ))
        .fetch_all(self.pool())
        .await?;

        rows.iter().map(subscription_from_row).collect()
    }

    pub async fn get_subscription(&self, id: &str) -> Result<Option<Subscription>> {
        let row = sqlx::query(&format!(
            "SELECT {SUBSCRIPTION_COLUMNS} FROM subscriptions WHERE id = ?"
        ))
        .bind(id)
        .fetch_optional(self.pool())
        .await?;

        row.as_ref().map(subscription_from_row).transpose()
    }

    pub async fn get_subscription_by_slug(&self, slug: &str) -> Result<Option<Subscription>> {
        let row = sqlx::query(&format!(
            "SELECT {SUBSCRIPTION_COLUMNS} FROM subscriptions WHERE slug = ?"
        ))
        .bind(slug)
        .fetch_optional(self.pool())
        .await?;

        row.as_ref().map(subscription_from_row).transpose()
    }

    pub async fn slugs(&self) -> Result<Vec<String>> {
        Ok(
            sqlx::query_scalar::<_, String>("SELECT slug FROM subscriptions ORDER BY slug")
                .fetch_all(self.pool())
                .await?,
        )
    }

    pub async fn create_subscription(&self, new: &NewSubscription) -> Result<Subscription> {
        let outbounds = bind_outbounds(&new.outbounds)?;

        sqlx::query(
            "INSERT INTO subscriptions (
                id, name, slug, source, url, user_agent, format, content, outbounds, enabled,
                auto_refresh, refresh_interval_secs, next_refresh_at, last_fetched_at, last_error,
                validated, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, '', ?, ?, ?)",
        )
        .bind(&new.id)
        .bind(&new.name)
        .bind(&new.slug)
        .bind(new.source.as_str())
        .bind(&new.url)
        .bind(&new.user_agent)
        .bind(&new.format)
        .bind(&new.content)
        .bind(outbounds)
        .bind(i64::from(new.enabled))
        .bind(i64::from(new.auto_refresh))
        .bind(new.refresh_interval_secs)
        .bind(new.next_refresh_at)
        .bind(new.now)
        .bind(i64::from(new.validated))
        .bind(new.now)
        .bind(new.now)
        .execute(self.pool())
        .await?;

        self.get_subscription(&new.id)
            .await?
            .ok_or(StoreError::NotFound)
    }

    pub async fn update_subscription(
        &self,
        id: &str,
        patch: &SubscriptionPatch,
    ) -> Result<Subscription> {
        let mut subscription = self
            .get_subscription(id)
            .await?
            .ok_or(StoreError::NotFound)?;

        if let Some(name) = &patch.name {
            subscription.name = name.clone();
        }
        if let Some(url) = &patch.url {
            subscription.url = Some(url.clone());
        }
        if let Some(user_agent) = &patch.user_agent {
            subscription.user_agent = user_agent.clone();
        }
        if let Some(enabled) = patch.enabled {
            subscription.enabled = enabled;
        }
        if let Some(auto_refresh) = patch.auto_refresh {
            subscription.auto_refresh = auto_refresh;
        }
        if let Some(interval) = patch.refresh_interval_secs {
            subscription.refresh_interval_secs = interval;
        }
        if let Some(next_refresh_at) = patch.next_refresh_at {
            subscription.next_refresh_at = Some(next_refresh_at);
        }
        subscription.updated_at = now();

        self.write_subscription(&subscription).await?;
        Ok(subscription)
    }

    pub async fn set_refresh_success(
        &self,
        id: &str,
        format: &str,
        content: &str,
        outbounds: &[serde_json::Value],
        fetched_at: i64,
        next_refresh_at: Option<i64>,
        validated: bool,
    ) -> Result<Subscription> {
        sqlx::query(
            "UPDATE subscriptions SET format = ?, content = ?, outbounds = ?, last_fetched_at = ?,
             next_refresh_at = ?, last_error = '', validated = ?, failures = 0, updated_at = ? WHERE id = ?",
        )
        .bind(format)
        .bind(content)
        .bind(bind_outbounds(outbounds)?)
        .bind(fetched_at)
        .bind(next_refresh_at)
        .bind(i64::from(validated))
        .bind(now())
        .bind(id)
        .execute(self.pool())
        .await?;

        self.get_subscription(id)
            .await?
            .ok_or(StoreError::NotFound)
    }

    pub async fn set_refresh_error(
        &self,
        id: &str,
        error: &str,
        next_refresh_at: Option<i64>,
    ) -> Result<i64> {
        sqlx::query(
            "UPDATE subscriptions SET last_error = ?, next_refresh_at = ?, failures = failures + 1, updated_at = ? WHERE id = ?",
        )
        .bind(error)
        .bind(next_refresh_at)
        .bind(now())
        .bind(id)
        .execute(self.pool())
        .await?;

        Ok(sqlx::query_scalar::<_, i64>("SELECT failures FROM subscriptions WHERE id = ?")
            .bind(id)
            .fetch_optional(self.pool())
            .await?
            .unwrap_or(0))
    }

    pub async fn delete_subscription(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM subscriptions WHERE id = ?")
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub async fn due_subscriptions(&self, at: i64) -> Result<Vec<Subscription>> {
        let rows = sqlx::query(&format!(
            "SELECT {SUBSCRIPTION_COLUMNS} FROM subscriptions
             WHERE enabled = 1 AND auto_refresh = 1 AND (next_refresh_at IS NULL OR next_refresh_at <= ?)
             ORDER BY next_refresh_at ASC"
        ))
        .bind(at)
        .fetch_all(self.pool())
        .await?;

        rows.iter().map(subscription_from_row).collect()
    }

    async fn write_subscription(&self, subscription: &Subscription) -> Result<()> {
        sqlx::query(
            "UPDATE subscriptions SET name = ?, url = ?, user_agent = ?, enabled = ?,
             auto_refresh = ?, refresh_interval_secs = ?, next_refresh_at = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(&subscription.name)
        .bind(&subscription.url)
        .bind(&subscription.user_agent)
        .bind(i64::from(subscription.enabled))
        .bind(i64::from(subscription.auto_refresh))
        .bind(subscription.refresh_interval_secs)
        .bind(subscription.next_refresh_at)
        .bind(subscription.updated_at)
        .bind(&subscription.id)
        .execute(self.pool())
        .await?;

        Ok(())
    }
}
