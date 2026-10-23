use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use sing_portal_core::protocol::{AgentMessage, ServerMessage};
use sing_portal_core::render::AUTO_TAG;
use sing_portal_store::NodeReport;
use tokio::sync::mpsc;

use crate::service;
use crate::state::AppState;

pub async fn upgrade(State(state): State<AppState>, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |socket| supervise(state, socket))
}

async fn supervise(state: AppState, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();

    let hello = match tokio::time::timeout(Duration::from_secs(10), stream.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => match serde_json::from_str::<AgentMessage>(&text) {
            Ok(AgentMessage::Hello { name, version }) => (name, version),
            _ => return,
        },
        _ => return,
    };
    let (name, version) = hello;
    if name.trim().is_empty() {
        return;
    }

    let (outbound, mut outbound_rx) = mpsc::channel::<ServerMessage>(32);
    state
        .registry()
        .register(&name, &version, outbound.clone())
        .await;
    if let Err(error) = state.store().seed_selection(&name, AUTO_TAG).await {
        tracing::warn!(%name, %error, "cannot seed machine selection");
    }
    tracing::info!(%name, %version, "agent connected");

    let mut writer = tokio::spawn(async move {
        while let Some(message) = outbound_rx.recv().await {
            let Ok(text) = serde_json::to_string(&message) else {
                break;
            };
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    if let Err(error) = service::push_state(&state, &name).await {
        tracing::warn!(%name, %error, "cannot push initial state");
    }

    loop {
        tokio::select! {
            frame = stream.next() => {
                match frame {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<AgentMessage>(&text) {
                            Ok(message) => dispatch(&state, &name, message).await,
                            Err(error) => tracing::warn!(%name, %error, "invalid agent message"),
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(error)) => {
                        tracing::debug!(%name, %error, "agent socket error");
                        break;
                    }
                    _ => {}
                }
            }
            _ = &mut writer => break,
        }
    }

    writer.abort();
    state.registry().unregister(&name).await;
    tracing::info!(%name, "agent disconnected");
}

async fn dispatch(state: &AppState, name: &str, message: AgentMessage) {
    match message {
        AgentMessage::Hello { .. } => {}
        AgentMessage::Status {
            revision: reported_revision,
            selection: reported_selection,
            health,
            error,
        } => {
            let report = NodeReport {
                node_name: name.to_string(),
                revision: reported_revision.clone(),
                selection: reported_selection.clone(),
                health,
                error,
                reported_at: sing_portal_store::now(),
            };
            if let Err(error) = service::record_report(state, report).await {
                tracing::warn!(%name, %error, "cannot store agent status");
            }

            if let Err(error) = service::push_state_if_changed(
                state,
                name,
                &reported_revision,
                &reported_selection,
            )
            .await
            {
                tracing::warn!(%name, %error, "cannot push state");
            }
        }
        AgentMessage::SyncResult {
            revision,
            selection,
            health,
            error,
            ..
        } => {
            let report = NodeReport {
                node_name: name.to_string(),
                revision,
                selection,
                health,
                error,
                reported_at: sing_portal_store::now(),
            };
            if let Err(error) = service::record_report(state, report).await {
                tracing::warn!(%name, %error, "cannot store sync result");
            }
        }
        other => {
            if let Some(id) = other.id().map(ToString::to_string) {
                let resolved = state.registry().resolve(&id, other).await;
                if !resolved {
                    tracing::debug!(%name, %id, "no pending request for agent reply");
                }
            }
        }
    }
}
