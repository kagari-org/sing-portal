use futures_util::StreamExt;
use std::cmp::Ordering;
use std::time::Duration;

use sing_portal_core::score::should_switch;
use sing_portal_store::{Measurement, SelectionMode};

use crate::error::ApiResult;
use crate::service;
use crate::state::AppState;

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

    for machine in service::known_machines(state).await? {
        if !state.registry().is_connected(&machine).await {
            continue;
        }
        if !state.begin_auto(&machine).await {
            continue;
        }
        let result = evaluate_machine(state, &machine).await;
        state.end_auto(&machine).await;

        match result {
            Ok(Some(evaluation)) => evaluations.push(evaluation),
            Ok(None) => {}
            Err(error) => tracing::warn!(machine = %machine, %error, "auto selection failed"),
        }
    }

    Ok(evaluations)
}

pub async fn evaluate_machine(state: &AppState, machine: &str) -> ApiResult<Option<Evaluation>> {
    if !state.registry().is_connected(machine).await {
        return Ok(None);
    }

    let Some(selection) = state.store().selection_with_mode(machine).await? else {
        return Ok(None);
    };
    if selection.mode != SelectionMode::Auto {
        return Ok(None);
    }

    let tags = service::active_tags(state).await?;
    if tags.is_empty() {
        return Ok(None);
    }

    let mut latency: Vec<(String, f64)> = futures_util::stream::iter(tags.iter().cloned().map(|tag| {
        let state = state.clone();
        let machine = machine.to_string();
        async move {
            match service::probe(&state, &machine, &tag, "latency").await {
                Ok(probe) => probe.latency_ms.map(|latency_ms| (tag, latency_ms)),
                Err(error) => {
                    tracing::debug!(%machine, %tag, %error, "auto latency probe failed");
                    None
                }
            }
        }
    }))
    .buffer_unordered(8)
    .filter_map(std::future::ready)
    .collect()
    .await;

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
        match service::probe(state, machine, tag, "both").await {
            Ok(probe) => {
                let score = state
                    .config()
                    .score
                    .score(probe.latency_ms.unwrap_or(f64::MAX), probe.down_mbps);
                measurements.push(Measurement {
                    node_name: machine.to_string(),
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
                tracing::debug!(%machine, %tag, %error, "auto probe failed");
                measurements.push(Measurement {
                    node_name: machine.to_string(),
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
        .replace_measurements(machine, &measurements)
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
        let applied = service::select(state, machine, &best_tag)
            .await
            .unwrap_or(false);
        state.store().set_selection(machine, &best_tag).await?;
        if let Err(push_error) = service::push_state(state, machine).await {
            tracing::warn!(%machine, %push_error, "cannot push auto selection");
        }
        state
            .store()
            .record_audit(
                "auto",
                "selection.auto",
                machine,
                &format!("tag={best_tag} score={best_score:.1} applied={applied}"),
            )
            .await?;
    }

    Ok(Some(Evaluation {
        machine: machine.to_string(),
        candidates: candidates.len(),
        best: Some(best_tag),
        score: Some(best_score),
        switched: switch,
    }))
}
