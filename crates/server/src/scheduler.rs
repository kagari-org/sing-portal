use std::time::Duration;

use crate::error::ApiResult;
use crate::service;
use crate::state::AppState;

pub async fn run(state: AppState) {
    let mut ticker = tokio::time::interval(Duration::from_secs(5));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        ticker.tick().await;
        match run_once(&state).await {
            Ok(0) => {}
            Ok(count) => tracing::info!(count, "auto-refreshed subscriptions"),
            Err(error) => tracing::warn!(%error, "scheduler tick failed"),
        }
    }
}

pub async fn run_once(state: &AppState) -> ApiResult<usize> {
    let due = state
        .store()
        .due_subscriptions(sing_portal_store::now())
        .await?;

    if due.is_empty() {
        return Ok(0);
    }

    let mut refreshed = 0;
    for subscription in due {
        if !state.begin_refresh(&subscription.id).await {
            continue;
        }
        let result = service::refresh_subscription(state, &subscription).await;
        state.end_refresh(&subscription.id).await;

        match result {
            Ok((updated, warnings)) => {
                refreshed += 1;
                if !warnings.is_empty() {
                    tracing::debug!(slug = %updated.slug, count = warnings.len(), "refresh warnings");
                }
            }
            Err(error) => {
                tracing::warn!(slug = %subscription.slug, %error, "subscription refresh failed");
            }
        }
    }

    if refreshed > 0 {
        let reset = service::reconcile_selections(state).await?;
        if !reset.is_empty() {
            tracing::info!(nodes = ?reset, "selections reset to s_auto");
        }

        service::broadcast_state(state).await;
        state.notify_changed(&["subscriptions", "providers", "revision", "nodes", "audit"]);

        let state = state.clone();
        tokio::spawn(async move {
            if let Err(error) = crate::auto::run_once(&state).await {
                tracing::warn!(%error, "auto selection after refresh failed");
            }
        });
    }

    Ok(refreshed)
}
