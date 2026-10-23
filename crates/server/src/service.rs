use std::time::Duration;

use serde_json::Value;
use sing_portal_core::canonical::{normalize, sort_by_tag};
use sing_portal_core::detect::{Format, parse_subscription};
use sing_portal_core::model::outbound_tag;
use sing_portal_core::render::{AUTO_TAG, DIRECT_TAG};
use sing_portal_store::{NodeReport, Source, Subscription};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::{fetch, validate};

pub struct Prepared {
    pub format: Format,
    pub content: String,
    pub outbounds: Vec<Value>,
    pub validated: bool,
    pub warnings: Vec<String>,
}

pub async fn prepare(
    state: &AppState,
    slug: &str,
    user_agent: &str,
    url: Option<&str>,
    content: Option<&str>,
) -> ApiResult<Prepared> {
    let bytes = match (url, content) {
        (Some(url), _) => fetch::fetch(state, url, user_agent).await?,
        (None, Some(content)) => content.as_bytes().to_vec(),
        (None, None) => {
            return Err(ApiError::BadRequest(
                "either url or content is required".into(),
            ));
        }
    };

    let parsed = parse_subscription(&bytes).map_err(ApiError::from_core)?;
    let outbounds = normalize(parsed.outbounds, slug).map_err(ApiError::from_core)?;
    let validated = validate::check(state, &outbounds).await?;

    Ok(Prepared {
        format: parsed.format,
        content: String::from_utf8_lossy(&bytes).into_owned(),
        outbounds,
        validated,
        warnings: parsed.warnings,
    })
}

pub async fn active_outbounds(state: &AppState) -> ApiResult<Vec<Value>> {
    let subscriptions = state.store().list_subscriptions().await?;
    let mut outbounds: Vec<Value> = subscriptions
        .iter()
        .filter(|subscription| subscription.enabled)
        .flat_map(|subscription| subscription.outbounds.iter().cloned())
        .collect();
    sort_by_tag(&mut outbounds);
    Ok(outbounds)
}

pub async fn active_tags(state: &AppState) -> ApiResult<Vec<String>> {
    Ok(active_outbounds(state)
        .await?
        .iter()
        .filter_map(outbound_tag)
        .map(ToString::to_string)
        .collect())
}

pub async fn revision(state: &AppState) -> ApiResult<String> {
    Ok(sing_portal_core::canonical::revision(
        &active_outbounds(state).await?,
    ))
}

pub fn next_refresh_at(interval: Duration) -> i64 {
    sing_portal_store::now() + interval.as_secs() as i64
}

pub fn backoff(failures: i64, interval: Duration) -> Duration {
    let base = match failures {
        0 | 1 => 300,
        2 => 900,
        3 => 3600,
        _ => 21600,
    };
    Duration::from_secs(base).min(interval)
}

pub fn interval_of(subscription: &Subscription) -> Duration {
    Duration::from_secs(subscription.refresh_interval_secs.clamp(60, 30 * 86400) as u64)
}

pub async fn refresh_subscription(
    state: &AppState,
    subscription: &Subscription,
) -> ApiResult<(Subscription, Vec<String>)> {
    let interval = interval_of(subscription);
    let content = if subscription.source == Source::Manual {
        Some(subscription.content.as_str())
    } else {
        None
    };

    match prepare(
        state,
        &subscription.slug,
        &subscription.user_agent,
        subscription.url.as_deref(),
        content,
    )
    .await
    {
        Ok(prepared) => {
            let next = if subscription.auto_refresh {
                Some(next_refresh_at(interval))
            } else {
                None
            };
            let updated = state
                .store()
                .set_refresh_success(
                    &subscription.id,
                    prepared.format.as_str(),
                    &prepared.content,
                    &prepared.outbounds,
                    sing_portal_store::now(),
                    next,
                    prepared.validated,
                )
                .await?;
            Ok((updated, prepared.warnings))
        }
        Err(error) => {
            let delay = backoff(subscription.failures, interval);
            let _ = state
                .store()
                .set_refresh_error(
                    &subscription.id,
                    error.message(),
                    Some(sing_portal_store::now() + delay.as_secs() as i64),
                )
                .await;
            Err(error)
        }
    }
}

pub async fn reconcile_selections(state: &AppState) -> ApiResult<Vec<String>> {
    let tags = active_tags(state).await?;
    let selections = state.store().selections().await?;
    let mut reset = Vec::new();

    for selection in selections {
        let keep = matches!(selection.outbound.as_str(), AUTO_TAG | DIRECT_TAG)
            || tags.iter().any(|tag| tag == &selection.outbound);
        if keep {
            continue;
        }
        state
            .store()
            .set_selection(&selection.node_name, AUTO_TAG)
            .await?;
        state
            .store()
            .record_audit(
                "ui",
                "selection.reset",
                &selection.node_name,
                "outbound no longer exists",
            )
            .await?;
        reset.push(selection.node_name);
    }

    Ok(reset)
}

pub async fn init_nodes(state: &AppState) -> ApiResult<()> {
    for node in &state.config().nodes {
        state.store().seed_selection(&node.name, AUTO_TAG).await?;
    }
    Ok(())
}

pub fn is_valid_choice(choice: &str, tags: &[String]) -> bool {
    matches!(choice, AUTO_TAG | DIRECT_TAG) || tags.iter().any(|tag| tag == choice)
}

pub async fn record_report(state: &AppState, report: NodeReport) -> ApiResult<bool> {
    let previous = state.store().report(&report.node_name).await?;
    let changed = previous
        .as_ref()
        .map(|previous| {
            previous.revision != report.revision
                || previous.selection != report.selection
                || previous.health != report.health
        })
        .unwrap_or(true);

    state.store().record_report(&report).await?;

    if changed {
        state
            .store()
            .record_audit(
                &format!("agent:{}", report.node_name),
                "node.report",
                &report.node_name,
                &format!("revision={} health={}", report.revision, report.health),
            )
            .await?;
    }

    Ok(changed)
}
