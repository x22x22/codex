use super::CHANNEL_CAPACITY;
use super::RemoteControlQueuedServerEnvelope;
use super::enroll::connect_remote_control_websocket;
use super::protocol::ClientEnvelope;
use super::protocol::ClientEvent;
use super::protocol::ClientId;
use super::protocol::RemoteControlTarget;
use super::protocol::ServerEnvelope;
use super::protocol::ServerEvent;
use codex_core::AuthManager;
use codex_state::StateRuntime;
use futures::SinkExt;
use futures::StreamExt;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::time::Duration;
use tokio_tungstenite::tungstenite;
use tokio_util::sync::CancellationToken;
use tracing::error;
use tracing::info;
use tracing::warn;

const REMOTE_CONTROL_RECONNECT_INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const REMOTE_CONTROL_RECONNECT_MAX_BACKOFF: Duration = Duration::from_secs(30);

enum RemoteControlWriteCommand {
    ServerEnvelope(RemoteControlQueuedServerEnvelope),
    Pong(tungstenite::Bytes),
}

struct BufferedServerEvent {
    event: ServerEvent,
    write_complete_tx: Option<oneshot::Sender<()>>,
}

#[allow(clippy::print_stderr)]
pub(super) async fn run_remote_control_websocket_loop(
    remote_control_target: RemoteControlTarget,
    state_db: Option<Arc<StateRuntime>>,
    auth_manager: Arc<AuthManager>,
    client_event_tx: mpsc::Sender<ClientEnvelope>,
    mut server_event_rx: mpsc::Receiver<RemoteControlQueuedServerEnvelope>,
    shutdown_token: CancellationToken,
) {
    let mut reconnect_backoff = REMOTE_CONTROL_RECONNECT_INITIAL_BACKOFF;
    let mut reconnect_attempt = 0_u64;
    let mut wait_before_connect = false;
    let mut enrollment = None;
    let mut outbound_buffer = HashMap::<ClientId, BTreeMap<u64, BufferedServerEvent>>::new();
    let mut subscribe_cursor: Option<String> = None;

    loop {
        if wait_before_connect {
            tokio::select! {
                _ = shutdown_token.cancelled() => break,
                _ = tokio::time::sleep(reconnect_backoff) => {}
            }
            reconnect_attempt = reconnect_attempt.saturating_add(1);
            warn!(
                "app-server remote control websocket reconnect attempt {reconnect_attempt} after {reconnect_backoff:?}"
            );
            reconnect_backoff = reconnect_backoff
                .saturating_mul(2)
                .min(REMOTE_CONTROL_RECONNECT_MAX_BACKOFF);
        } else {
            wait_before_connect = true;
        }

        let websocket_connection = tokio::select! {
            _ = shutdown_token.cancelled() => break,
            connect_result = connect_remote_control_websocket(
                &remote_control_target,
                state_db.as_deref(),
                auth_manager.as_ref(),
                &mut enrollment,
                subscribe_cursor.as_deref(),
            ) => {
                match connect_result {
                    Ok(websocket_connection) => {
                        reconnect_backoff = REMOTE_CONTROL_RECONNECT_INITIAL_BACKOFF;
                        reconnect_attempt = 0;
                        info!(
                            "connected to app-server remote control websocket: {}",
                            remote_control_target.websocket_url
                        );
                        websocket_connection
                    }
                    Err(err) => {
                        warn!("{err}");
                        continue;
                    }
                }
            }
        };

        let (mut websocket_writer, mut websocket_reader) =
            websocket_connection.websocket_stream.split();
        let (write_command_tx, mut write_command_rx) = mpsc::channel(CHANNEL_CAPACITY);
        let (reader_event_tx, mut reader_event_rx) = mpsc::channel(CHANNEL_CAPACITY);

        let mut buffered_events_to_resend = Vec::new();
        for (client_id, buffered_events) in outbound_buffer.iter_mut() {
            for (seq_id, buffered_event) in buffered_events.iter_mut() {
                buffered_events_to_resend.push(RemoteControlQueuedServerEnvelope {
                    envelope: ServerEnvelope {
                        event: buffered_event.event.clone(),
                        client_id: client_id.clone(),
                        seq_id: Some(*seq_id),
                    },
                    write_complete_tx: buffered_event.write_complete_tx.take(),
                });
            }
        }
        let mut write_task = tokio::spawn(async move {
            for server_envelope in buffered_events_to_resend {
                let payload = match serde_json::to_string(&server_envelope.envelope) {
                    Ok(payload) => payload,
                    Err(err) => {
                        error!("failed to serialize remote-control server event: {err}");
                        continue;
                    }
                };
                if websocket_writer
                    .send(tungstenite::Message::Text(payload.into()))
                    .await
                    .is_err()
                {
                    return;
                }
                if let Some(write_complete_tx) = server_envelope.write_complete_tx {
                    let _ = write_complete_tx.send(());
                }
            }

            while let Some(write_command) = write_command_rx.recv().await {
                match write_command {
                    RemoteControlWriteCommand::ServerEnvelope(server_envelope) => {
                        let payload = match serde_json::to_string(&server_envelope.envelope) {
                            Ok(payload) => payload,
                            Err(err) => {
                                error!("failed to serialize remote-control server event: {err}");
                                continue;
                            }
                        };
                        if websocket_writer
                            .send(tungstenite::Message::Text(payload.into()))
                            .await
                            .is_err()
                        {
                            return;
                        }
                        if let Some(write_complete_tx) = server_envelope.write_complete_tx {
                            let _ = write_complete_tx.send(());
                        }
                    }
                    RemoteControlWriteCommand::Pong(payload) => {
                        if websocket_writer
                            .send(tungstenite::Message::Pong(payload))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                }
            }
        });

        let write_command_tx_for_reader = write_command_tx.clone();
        let mut read_task = tokio::spawn(async move {
            while let Some(incoming_message) = websocket_reader.next().await {
                match incoming_message {
                    Ok(tungstenite::Message::Text(text)) => {
                        if let Ok(client_envelope) = serde_json::from_str::<ClientEnvelope>(&text) {
                            if reader_event_tx.send(client_envelope).await.is_err() {
                                return;
                            }
                        } else {
                            warn!("failed to deserialize remote-control client event");
                        }
                    }
                    Ok(tungstenite::Message::Ping(payload)) => {
                        if write_command_tx_for_reader
                            .send(RemoteControlWriteCommand::Pong(payload))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    Ok(tungstenite::Message::Pong(_)) => {}
                    Ok(tungstenite::Message::Binary(_)) => {
                        warn!("dropping unsupported binary remote-control websocket message");
                    }
                    Ok(tungstenite::Message::Frame(_)) => {}
                    Ok(tungstenite::Message::Close(_)) => {
                        warn!("remote control websocket disconnected");
                        return;
                    }
                    Err(err) => {
                        warn!("remote control websocket receive failed: {err}");
                        return;
                    }
                }
            }
            warn!("remote control websocket disconnected");
        });

        loop {
            tokio::select! {
                _ = shutdown_token.cancelled() => {
                    write_task.abort();
                    read_task.abort();
                    return;
                }
                _ = &mut write_task => {
                    read_task.abort();
                    break;
                }
                _ = &mut read_task => {
                    write_task.abort();
                    break;
                }
                client_envelope = reader_event_rx.recv() => {
                    let Some(client_envelope) = client_envelope else {
                        write_task.abort();
                        read_task.abort();
                        break;
                    };
                    if let Some(cursor) = client_envelope.cursor.as_deref() {
                        subscribe_cursor = Some(cursor.to_string());
                    }
                    if let ClientEvent::Ack { acked_seq_id } = &client_envelope.event
                        && let Some(buffered_events) = outbound_buffer.get_mut(&client_envelope.client_id)
                    {
                        let acknowledged_seq_ids: Vec<u64> = buffered_events
                            .range(..=*acked_seq_id)
                            .map(|(seq_id, _)| *seq_id)
                            .collect();
                        for acknowledged_seq_id in acknowledged_seq_ids {
                            buffered_events.remove(&acknowledged_seq_id);
                        }
                        if buffered_events.is_empty() {
                            outbound_buffer.remove(&client_envelope.client_id);
                        }
                    }
                    if client_event_tx.send(client_envelope).await.is_err() {
                        write_task.abort();
                        read_task.abort();
                        return;
                    }
                }
                server_envelope = server_event_rx.recv() => {
                    let Some(server_envelope) = server_envelope else {
                        write_task.abort();
                        read_task.abort();
                        return;
                    };
                    if let ServerEvent::ServerMessage { .. } = &server_envelope.envelope.event
                        && let Some(seq_id) = server_envelope.envelope.seq_id
                    {
                        outbound_buffer
                            .entry(server_envelope.envelope.client_id.clone())
                            .or_default()
                            .insert(seq_id, BufferedServerEvent {
                                event: server_envelope.envelope.event.clone(),
                                write_complete_tx: None,
                            });
                    }
                    if let Err(err) = write_command_tx
                        .send(RemoteControlWriteCommand::ServerEnvelope(server_envelope))
                        .await
                    {
                        let RemoteControlWriteCommand::ServerEnvelope(server_envelope) = err.0 else {
                            unreachable!();
                        };
                        if let ServerEvent::ServerMessage { .. } = &server_envelope.envelope.event
                            && let Some(seq_id) = server_envelope.envelope.seq_id
                            && let Some(buffered_events) = outbound_buffer.get_mut(&server_envelope.envelope.client_id)
                            && let Some(buffered_event) = buffered_events.get_mut(&seq_id)
                        {
                            buffered_event.write_complete_tx = server_envelope.write_complete_tx;
                        }
                        write_task.abort();
                        read_task.abort();
                        break;
                    }
                }
            }
        }
    }
}
