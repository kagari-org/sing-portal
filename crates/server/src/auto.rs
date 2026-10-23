use std::cmp::Ordering;
use std::time::Duration;

use sing_portal_core::clash_api::ClashClient;
use sing_portal_core::render::SELECTOR_TAG;
use sing_portal_core::score::should_switch;
use sing_portal_store::{Measurement, SelectionMode};

use crate::config::NodeConfig;
use crate::error::ApiResult;
use crate::state::AppState;

use crate::service;

#[derive(Debug, Clone)]
pub struct Evaluation {
    pub machine: String,
    pub candidates: usize,
    pub best: Option<String>,
    pub score: Option<f64>,
    pub switched: bool,
}

pub async fn run(state: AppState) {
    let interval = state.config().auto_interval;
    tokio::time::sleep(interval.min(Duration::from_secs(30))).await;
    if let Err(error) = run_once(&state).await {
        tracing::warn!(%error, "initial auto selection cycle failed");
    }

    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await;

    loop {
        ticker.tick().await;
        match run_once(&state).await {
            Ok(evaluations) => {
                for evaluation in evaluations {
                    if evaluation.switched {
                        tracing::info!(
                            machine = %evaluation.machine,
                            best = ?evaluation.best,
                            score = ?evaluation.score,
                            "auto selection switched"
                        );
                    }
                }
            }
            Err(error) => tracing::warn!(%error, "auto selection cycle failed"),
        }
    }
}

pub async fn run_once(state: &AppState) -> ApiResult<Vec<Evaluation>> {
    let mut evaluations = Vec::new();

    for node in state.config().nodes.clone() {
        if !state.begin_auto(&node.name).await {
            continue;
        }
        let result = evaluate_machine(state, &node).await;
        state.end_auto(&node.name).await;

        match result {
            Ok(Some(evaluation)) => evaluations.push(evaluation),
            Ok(None) => {}
            Err(error) => tracing::warn!(machine = %node.name, %error, "auto selection failed"),
        }
    }

    Ok(evaluations)
}

pub async fn evaluate_machine(
    state: &AppState,
    node: &NodeConfig,
) -> ApiResult<Option<Evaluation>> {
    let Some(selection) = state.store().selection_with_mode(&node.name).await? else {
        return Ok(None);
    };
    if selection.mode != SelectionMode::Auto {
        return Ok(None);
    }

    let tags = service::active_tags(state).await?;
    if tags.is_empty() {
        return Ok(None);
    }

    let mut latency: Vec<(String, f64)> = Vec::new();
    for tag in &tags {
        if let Ok(probe) = service::probe(state, node, tag, "latency", None).await
            && let Some(latency_ms) = probe.latency_ms
        {
            latency.push((tag.clone(), latency_ms));
        }
    }
    if latency.is_empty() {
        return Ok(None);
    }
    latency.sort_by(|left, right| {
        left.1
            .partial_cmp(&right.1)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.0.cmp(&right.0))
    });

    let mut candidates: Vec<String> = latency
        .iter()
        .take(state.config().auto_candidates.max(1))
        .map(|(tag, _)| tag.clone())
        .collect();
    if tags.contains(&selection.outbound) && !candidates.contains(&selection.outbound) {
        candidates.push(selection.outbound.clone());
    }

    let measured_at = sing_portal_store::now();
    let mut measurements = Vec::with_capacity(candidates.len());
    let mut scored: Vec<(String, f64, Option<f64>, Option<f64>)> = Vec::new();

    for tag in &candidates {
        match service::probe(state, node, tag, "both", None).await {
            Ok(probe) => {
                let score = state
                    .config()
                    .score
                    .score(probe.latency_ms.unwrap_or(f64::MAX), probe.down_mbps);
                measurements.push(Measurement {
                    node_name: node.name.clone(),
                    tag: tag.clone(),
                    latency_ms: probe.latency_ms,
                    down_mbps: probe.down_mbps,
                    score,
                    measured_at,
                });
                if score > 0.0 {
                    scored.push((tag.clone(), score, probe.latency_ms, probe.down_mbps));
                }
            }
            Err(error) => {
                tracing::debug!(machine = %node.name, %tag, %error, "auto probe failed");
                measurements.push(Measurement {
                    node_name: node.name.clone(),
                    tag: tag.clone(),
                    latency_ms: None,
                    down_mbps: None,
                    score: 0.0,
                    measured_at,
                });
            }
        }
    }

    state
        .store()
        .replace_measurements(&node.name, &measurements)
        .await?;

    if scored.is_empty() {
        return Ok(None);
    }

    scored.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(Ordering::Equal)
            .then_with(|| {
                right
                    .3
                    .unwrap_or(0.0)
                    .partial_cmp(&left.3.unwrap_or(0.0))
                    .unwrap_or(Ordering::Equal)
            })
            .then_with(|| {
                left.2
                    .unwrap_or(f64::MAX)
                    .partial_cmp(&right.2.unwrap_or(f64::MAX))
                    .unwrap_or(Ordering::Equal)
            })
            .then_with(|| left.0.cmp(&right.0))
    });

    let (best_tag, best_score, ..) = scored[0].clone();
    let current_score = scored
        .iter()
        .find(|(tag, ..)| tag == &selection.outbound)
        .map(|(_, score, ..)| *score)
        .unwrap_or(0.0);
    let current_usable = scored
        .iter()
        .any(|(tag, ..)| tag == &selection.outbound);

    let switch = !current_usable
        || should_switch(best_score, current_score, state.config().auto_margin);

    if switch {
        let applied = ClashClient::new(&node.clash_base_url(), Duration::from_secs(3))
            .map_err(crate::error::ApiError::from_core)?
            .select(SELECTOR_TAG, &best_tag)
            .await
            .is_ok();

        state.store().set_selection(&node.name, &best_tag).await?;
        state
            .store()
            .record_audit(
                "auto",
                "selection.auto",
                &node.name,
                &format!("tag={best_tag} score={best_score:.1} applied={applied}"),
            )
            .await?;
    }

    Ok(Some(Evaluation {
        machine: node.name.clone(),
        candidates: candidates.len(),
        best: Some(best_tag),
        score: Some(best_score),
        switched: switch,
    }))
}
