use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::time::Duration;

use futures_util::StreamExt;
use sing_portal_core::score::should_switch;
use sing_portal_store::{Measurement, SelectionMode};

use crate::error::ApiResult;
use crate::service;
use crate::state::AppState;

const RECOVERY_CANDIDATES: usize = 5;
const RECOVERY_COOLDOWN_SECS: i64 = 60;

#[derive(Debug, Clone)]
pub struct Evaluation {
    pub machine: String,
    pub candidates: usize,
    pub best: Option<String>,
    pub score: Option<f64>,
    pub switched: bool,
    pub checked: usize,
    pub error: String,
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

        match evaluate_locked(state, &machine, None).await {
            Ok(Some(evaluation)) => evaluations.push(evaluation),
            Ok(None) => {}
            Err(error) => tracing::warn!(machine = %machine, %error, "auto selection failed"),
        }
    }

    Ok(evaluations)
}

/// Start a full evaluation in the background. Returns false when one is
/// already running for this machine.
pub async fn trigger(state: &AppState, machine: &str) -> bool {
    trigger_with_limit(state, machine, None).await
}

/// Watchdog entry point: when the real egress is reported broken, run a
/// smaller evaluation immediately instead of waiting for the next cycle.
pub async fn trigger_recovery(state: &AppState, machine: &str) -> bool {
    if !state.registry().is_connected(machine).await {
        return false;
    }

    let now = sing_portal_store::now();
    if let Some(evaluation) = state.evaluation(machine).await {
        if evaluation.running {
            return false;
        }
        let last = evaluation.finished_at.unwrap_or(evaluation.started_at);
        if now.saturating_sub(last) < RECOVERY_COOLDOWN_SECS {
            return false;
        }
    }

    let limit = state.config().auto_candidates.clamp(1, RECOVERY_CANDIDATES);
    trigger_with_limit(state, machine, Some(limit)).await
}

async fn trigger_with_limit(
    state: &AppState,
    machine: &str,
    candidate_limit: Option<usize>,
) -> bool {
    if !state.begin_auto(machine).await {
        return false;
    }

    let state = state.clone();
    let machine = machine.to_string();
    tokio::spawn(async move {
        match evaluate_locked(&state, &machine, candidate_limit).await {
            Ok(Some(evaluation)) => tracing::info!(
                machine = %machine,
                switched = evaluation.switched,
                best = ?evaluation.best,
                score = ?evaluation.score,
                checked = evaluation.checked,
                "auto selection evaluated"
            ),
            Ok(None) => {}
            Err(error) => tracing::warn!(machine = %machine, %error, "auto selection failed"),
        }
    });

    true
}

/// Run a full evaluation for `machine`. The caller must already hold the
/// evaluation slot from `begin_auto`; this records progress and releases it.
pub async fn evaluate_locked(
    state: &AppState,
    machine: &str,
    candidate_limit: Option<usize>,
) -> ApiResult<Option<Evaluation>> {
    let result = evaluate_machine(state, machine, candidate_limit).await;

    match &result {
        Ok(Some(evaluation)) => {
            let error = if evaluation.best.is_none() && evaluation.error.is_empty() {
                "no usable candidate".to_string()
            } else {
                evaluation.error.clone()
            };
            state
                .finish_evaluation(
                    machine,
                    evaluation.best.clone(),
                    evaluation.score,
                    evaluation.switched,
                    evaluation.checked,
                    &error,
                )
                .await;
        }
        Ok(None) => {
            state
                .finish_evaluation(machine, None, None, false, 0, "evaluation skipped")
                .await;
        }
        Err(error) => {
            state
                .finish_evaluation(machine, None, None, false, 0, error.message())
                .await;
        }
    }

    state.end_auto(machine).await;
    result
}

pub async fn evaluate_machine(
    state: &AppState,
    machine: &str,
    candidate_limit: Option<usize>,
) -> ApiResult<Option<Evaluation>> {
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

    state.store().clear_measurements(machine).await?;
    state.notify_changed(&["measurements", "nodes"]);

    let total_tags = tags.len();
    state
        .update_evaluation(machine, "latency", 0, total_tags, None)
        .await;

    let done = Arc::new(AtomicUsize::new(0));
    let mut latency: Vec<(String, f64)> =
        futures_util::stream::iter(tags.iter().cloned().map(|tag| {
            let state = state.clone();
            let machine = machine.to_string();
            let done = done.clone();
            async move {
                let (latency_ms, ranked) =
                    match service::probe(&state, &machine, &tag, "latency").await {
                        Ok(probe) => (
                            probe.latency_ms,
                            probe.latency_ms.map(|latency_ms| (tag.clone(), latency_ms)),
                        ),
                        Err(error) => {
                            tracing::debug!(%machine, %tag, %error, "auto latency probe failed");
                            (None, None)
                        }
                    };
                let measurement = Measurement {
                    node_name: machine.clone(),
                    tag: tag.clone(),
                    latency_ms,
                    down_mbps: None,
                    score: 0.0,
                    measured_at: sing_portal_store::now(),
                };
                if let Err(error) = state.store().upsert_measurement(&measurement).await {
                    tracing::warn!(%machine, %tag, %error, "cannot store latency measurement");
                }
                state.notify_changed(&["measurements", "nodes"]);
                let index = done.fetch_add(1, AtomicOrdering::SeqCst) + 1;
                state
                    .update_evaluation(&machine, "latency", index, total_tags, Some(&tag))
                    .await;
                ranked
            }
        }))
        .buffer_unordered(8)
        .filter_map(std::future::ready)
        .collect()
        .await;

    if latency.is_empty() {
        return Ok(Some(failed(machine, 0, "no latency probe succeeded")));
    }
    let baseline: HashMap<String, f64> = latency.iter().cloned().collect();
    latency.sort_by(|left, right| {
        left.1
            .partial_cmp(&right.1)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.0.cmp(&right.0))
    });

    let max_candidates = candidate_limit
        .unwrap_or(state.config().auto_candidates)
        .max(1);
    let mut candidates: Vec<String> = latency
        .iter()
        .take(max_candidates)
        .map(|(tag, _)| tag.clone())
        .collect();
    if tags.contains(&selection.outbound) && !candidates.contains(&selection.outbound) {
        candidates.push(selection.outbound.clone());
    }

    let total_candidates = candidates.len();
    state
        .update_evaluation(machine, "bandwidth", 0, total_candidates, None)
        .await;

    let mut scored: Vec<(String, f64, Option<f64>, Option<f64>)> = Vec::new();

    for (index, tag) in candidates.iter().enumerate() {
        state
            .update_evaluation(machine, "bandwidth", index, total_candidates, Some(tag))
            .await;
        match service::probe(state, machine, tag, "both").await {
            Ok(probe) => {
                let latency_ms = probe.latency_ms.or_else(|| baseline.get(tag).copied());
                let score = state
                    .config()
                    .score
                    .score(latency_ms.unwrap_or(f64::MAX), probe.down_mbps);
                let measurement = Measurement {
                    node_name: machine.to_string(),
                    tag: tag.clone(),
                    latency_ms,
                    down_mbps: probe.down_mbps,
                    score,
                    measured_at: sing_portal_store::now(),
                };
                if let Err(error) = state.store().upsert_measurement(&measurement).await {
                    tracing::warn!(%machine, %tag, %error, "cannot store bandwidth measurement");
                }
                state.notify_changed(&["measurements", "nodes"]);
                if score > 0.0 {
                    scored.push((tag.clone(), score, latency_ms, probe.down_mbps));
                }
            }
            Err(error) => {
                tracing::debug!(%machine, %tag, %error, "auto probe failed");
                let measurement = Measurement {
                    node_name: machine.to_string(),
                    tag: tag.clone(),
                    latency_ms: baseline.get(tag).copied(),
                    down_mbps: None,
                    score: 0.0,
                    measured_at: sing_portal_store::now(),
                };
                if let Err(error) = state.store().upsert_measurement(&measurement).await {
                    tracing::warn!(%machine, %tag, %error, "cannot store failed bandwidth probe");
                }
                state.notify_changed(&["measurements", "nodes"]);
            }
        }
    }
    state
        .update_evaluation(
            machine,
            "bandwidth",
            total_candidates,
            total_candidates,
            None,
        )
        .await;

    if scored.is_empty() {
        return Ok(Some(failed(
            machine,
            candidates.len(),
            "no candidate passed the probes",
        )));
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

    // The egress probe runs through s_test and a dedicated probe DNS server,
    // so a candidate can be validated (including DNS) while s_out keeps
    // serving traffic. Only the final winner is applied, once.
    let max_checks = scored.len().min(3);
    state
        .update_evaluation(machine, "verify", 0, max_checks, None)
        .await;

    let mut chosen: Option<(String, f64)> = None;
    let mut checked = 0usize;
    for (index, (tag, score, ..)) in scored.iter().take(max_checks).enumerate() {
        if state
            .store()
            .selection_with_mode(machine)
            .await?
            .map(|selection| selection.mode)
            != Some(SelectionMode::Auto)
        {
            return Ok(Some(failed(
                machine,
                candidates.len(),
                "selection mode changed during evaluation",
            )));
        }

        checked = index + 1;
        state
            .update_evaluation(machine, "verify", index, max_checks, Some(tag))
            .await;

        match service::probe(state, machine, tag, "egress").await {
            Ok(check) if check.latency_ms.is_some() => {
                chosen = Some((tag.clone(), *score));
                break;
            }
            Ok(_) => tracing::debug!(%machine, %tag, "egress check returned no latency"),
            Err(error) => tracing::debug!(%machine, %tag, %error, "egress check failed"),
        }
    }
    state
        .update_evaluation(machine, "verify", checked, max_checks, None)
        .await;

    let Some((chosen_tag, chosen_score)) = chosen else {
        return Ok(Some(Evaluation {
            machine: machine.to_string(),
            candidates: candidates.len(),
            best: None,
            score: None,
            switched: false,
            checked,
            error: "every verified candidate failed the egress check".to_string(),
        }));
    };

    let current_score = scored
        .iter()
        .find(|(tag, ..)| tag == &selection.outbound)
        .map(|(_, score, ..)| *score)
        .unwrap_or(0.0);
    let current_usable = scored.iter().any(|(tag, ..)| tag == &selection.outbound);
    let switched = chosen_tag != selection.outbound
        && (!current_usable
            || should_switch(chosen_score, current_score, state.config().auto_margin));

    if switched {
        let applied = service::select(state, machine, &chosen_tag)
            .await
            .unwrap_or(false);
        state.store().set_selection(machine, &chosen_tag).await?;
        if let Err(push_error) = service::push_state(state, machine).await {
            tracing::warn!(%machine, %push_error, "cannot push auto selection");
        }
        state
            .store()
            .record_audit(
                "auto",
                "selection.auto",
                machine,
                &format!(
                    "tag={chosen_tag} score={chosen_score:.1} checked={checked} applied={applied}"
                ),
            )
            .await?;
        state.notify_changed(&["nodes", "audit"]);
    }

    Ok(Some(Evaluation {
        machine: machine.to_string(),
        candidates: candidates.len(),
        best: Some(chosen_tag),
        score: Some(chosen_score),
        switched,
        checked,
        error: String::new(),
    }))
}

fn failed(machine: &str, candidates: usize, error: &str) -> Evaluation {
    Evaluation {
        machine: machine.to_string(),
        candidates,
        best: None,
        score: None,
        switched: false,
        checked: 0,
        error: error.to_string(),
    }
}
