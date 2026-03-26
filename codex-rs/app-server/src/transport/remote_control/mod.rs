mod enroll;
mod protocol;
mod websocket;

use self::enroll::load_remote_control_auth;
use self::protocol::ClientEnvelope;
pub use self::protocol::ClientEvent;
pub use self::protocol::ClientId;
use self::protocol::PongStatus;
use self::protocol::ServerEnvelope;
use self::protocol::ServerEvent;
use self::protocol::normalize_remote_control_url;
use self::websocket::run_remote_control_websocket_loop;
use super::CHANNEL_CAPACITY;
use super::TransportEvent;
use super::next_connection_id;
use crate::outgoing_message::ConnectionId;
use crate::outgoing_message::QueuedOutgoingMessage;
use codex_app_server_protocol::JSONRPCMessage;
use codex_core::AuthManager;
use codex_state::StateRuntime;
use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::task::JoinSet;
use tokio::time::Duration;
use tokio::time::Instant;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

const REMOTE_CONTROL_CLIENT_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const REMOTE_CONTROL_IDLE_SWEEP_INTERVAL: Duration = Duration::from_secs(30);

struct RemoteControlClientState {
    connection_id: ConnectionId,
    disconnect_token: CancellationToken,
    last_activity_at: Instant,
    last_inbound_seq_id: Option<u64>,
}

pub(super) struct RemoteControlQueuedServerEnvelope {
    pub(super) envelope: ServerEnvelope,
    pub(super) write_complete_tx: Option<oneshot::Sender<()>>,
}

pub(crate) async fn start_remote_control(
    remote_control_url: String,
    state_db: Option<Arc<StateRuntime>>,
    auth_manager: Arc<AuthManager>,
    transport_event_tx: mpsc::Sender<TransportEvent>,
    shutdown_token: CancellationToken,
) -> io::Result<JoinHandle<()>> {
    let remote_control_url = normalize_remote_control_url(&remote_control_url)?;
    Ok(tokio::spawn(async move {
        let local_shutdown_token = shutdown_token.child_token();
        let (client_event_tx, client_event_rx) = mpsc::channel(CHANNEL_CAPACITY);
        let (server_event_tx, server_event_rx) = mpsc::channel(CHANNEL_CAPACITY);
        let (writer_exited_tx, writer_exited_rx) = mpsc::channel(CHANNEL_CAPACITY);

        let mut join_set = JoinSet::new();
        join_set.spawn(run_remote_control_websocket_loop(
            remote_control_url,
            state_db,
            auth_manager,
            client_event_tx,
            server_event_rx,
            local_shutdown_token.clone(),
        ));
        join_set.spawn(run_remote_control_manager(
            transport_event_tx,
            client_event_rx,
            server_event_tx,
            writer_exited_tx,
            writer_exited_rx,
            local_shutdown_token.clone(),
        ));

        tokio::select! {
            _ = local_shutdown_token.cancelled() => {}
            _ = join_set.join_next() => local_shutdown_token.cancel(),
        }

        join_set.shutdown().await;
    }))
}

async fn run_remote_control_manager(
    transport_event_tx: mpsc::Sender<TransportEvent>,
    mut client_event_rx: mpsc::Receiver<ClientEnvelope>,
    server_event_tx: mpsc::Sender<RemoteControlQueuedServerEnvelope>,
    writer_exited_tx: mpsc::Sender<ClientId>,
    mut writer_exited_rx: mpsc::Receiver<ClientId>,
    shutdown_token: CancellationToken,
) {
    let mut clients = HashMap::<ClientId, RemoteControlClientState>::new();
    let mut idle_sweep = tokio::time::interval(REMOTE_CONTROL_IDLE_SWEEP_INTERVAL);
    idle_sweep.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = shutdown_token.cancelled() => {
                break;
            }
            _ = idle_sweep.tick() => {
                if !close_expired_remote_control_clients(&transport_event_tx, &mut clients).await {
                    break;
                }
            }
            writer_exited = writer_exited_rx.recv() => {
                let Some(client_id) = writer_exited else {
                    break;
                };
                if !close_remote_control_client(&transport_event_tx, &mut clients, &client_id).await {
                    break;
                }
            }
            client_event = client_event_rx.recv() => {
                let Some(client_event) = client_event else {
                    break;
                };
                match client_event.event {
                    ClientEvent::ClientMessage { message } => {
                        let client_id = client_event.client_id;
                        let is_initialize = remote_control_message_starts_connection(&message);
                        if let Some(seq_id) = client_event.seq_id
                            && let Some(client) = clients.get(&client_id)
                                && client.last_inbound_seq_id.is_some_and(|last_seq_id| last_seq_id >= seq_id)
                                && !is_initialize
                            {
                                continue;
                            }

                        if is_initialize && clients.contains_key(&client_id)
                            && !close_remote_control_client(&transport_event_tx, &mut clients, &client_id).await {
                                break;
                            }

                        if let Some(connection_id) = clients.get_mut(&client_id).map(|client| {
                            client.last_activity_at = Instant::now();
                            if let Some(seq_id) = client_event.seq_id {
                                client.last_inbound_seq_id = Some(seq_id);
                            }
                            client.connection_id
                        }) {
                            if transport_event_tx
                                .send(TransportEvent::IncomingMessage {
                                    connection_id,
                                    message,
                                })
                                .await
                                .is_err()
                            {
                                break;
                            }
                            continue;
                        }

                        if !is_initialize {
                            continue;
                        }

                        let connection_id = next_connection_id();
                        let (writer_tx, writer_rx) =
                            mpsc::channel::<QueuedOutgoingMessage>(CHANNEL_CAPACITY);
                        let disconnect_token = CancellationToken::new();
                        if transport_event_tx
                            .send(TransportEvent::ConnectionOpened {
                                connection_id,
                                writer: writer_tx,
                                disconnect_sender: Some(disconnect_token.clone()),
                            })
                            .await
                            .is_err()
                        {
                            break;
                        }

                        tokio::spawn(run_remote_control_client_outbound(
                            client_id.clone(),
                            writer_rx,
                            server_event_tx.clone(),
                            writer_exited_tx.clone(),
                            disconnect_token.clone(),
                        ));
                        clients.insert(
                            client_id,
                            RemoteControlClientState {
                                connection_id,
                                disconnect_token,
                                last_activity_at: Instant::now(),
                                last_inbound_seq_id: client_event.seq_id,
                            },
                        );
                        if transport_event_tx
                            .send(TransportEvent::IncomingMessage {
                                connection_id,
                                message,
                            })
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    ClientEvent::Ack { .. } => continue,
                    ClientEvent::Ping => {
                        let client_id = client_event.client_id;
                        let status = match clients.get_mut(&client_id) {
                            Some(client) => {
                                client.last_activity_at = Instant::now();
                                PongStatus::Active
                            }
                            None => PongStatus::Unknown,
                        };

                        if server_event_tx
                            .send(RemoteControlQueuedServerEnvelope {
                                envelope: ServerEnvelope {
                                    event: ServerEvent::Pong { status },
                                    client_id,
                                    seq_id: None,
                                },
                                write_complete_tx: None,
                            })
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    ClientEvent::ClientClosed => {
                        let client_id = client_event.client_id;
                        if !close_remote_control_client(&transport_event_tx, &mut clients, &client_id).await {
                            break;
                        }
                    }
                }
            }
        }
    }

    while let Some(client_id) = clients.keys().next().cloned() {
        if !close_remote_control_client(&transport_event_tx, &mut clients, &client_id).await {
            break;
        }
    }
}

fn remote_control_message_starts_connection(message: &JSONRPCMessage) -> bool {
    matches!(
        message,
        JSONRPCMessage::Request(codex_app_server_protocol::JSONRPCRequest { method, .. })
            if method == "initialize"
    )
}

fn remote_control_client_is_alive(client: &RemoteControlClientState, now: Instant) -> bool {
    now.duration_since(client.last_activity_at) < REMOTE_CONTROL_CLIENT_IDLE_TIMEOUT
}

async fn close_expired_remote_control_clients(
    transport_event_tx: &mpsc::Sender<TransportEvent>,
    clients: &mut HashMap<ClientId, RemoteControlClientState>,
) -> bool {
    let now = Instant::now();
    let expired_client_ids: Vec<ClientId> = clients
        .iter()
        .filter_map(|(client_id, client)| {
            (!remote_control_client_is_alive(client, now)).then_some(client_id.clone())
        })
        .collect();
    for client_id in expired_client_ids {
        if !close_remote_control_client(transport_event_tx, clients, &client_id).await {
            return false;
        }
    }
    true
}

async fn close_remote_control_client(
    transport_event_tx: &mpsc::Sender<TransportEvent>,
    clients: &mut HashMap<ClientId, RemoteControlClientState>,
    client_id: &ClientId,
) -> bool {
    let Some(client) = clients.remove(client_id) else {
        return true;
    };
    client.disconnect_token.cancel();
    transport_event_tx
        .send(TransportEvent::ConnectionClosed {
            connection_id: client.connection_id,
        })
        .await
        .is_ok()
}

async fn run_remote_control_client_outbound(
    client_id: ClientId,
    mut writer_rx: mpsc::Receiver<QueuedOutgoingMessage>,
    server_event_tx: mpsc::Sender<RemoteControlQueuedServerEnvelope>,
    writer_exited_tx: mpsc::Sender<ClientId>,
    disconnect_token: CancellationToken,
) {
    let mut seq_id = 0_u64;
    loop {
        tokio::select! {
            _ = disconnect_token.cancelled() => {
                break;
            }
            queued_message = writer_rx.recv() => {
                let Some(queued_message) = queued_message else {
                    break;
                };
                if server_event_tx
                    .send(RemoteControlQueuedServerEnvelope {
                        envelope: ServerEnvelope {
                            event: ServerEvent::ServerMessage {
                                message: Box::new(queued_message.message),
                            },
                            client_id: client_id.clone(),
                            seq_id: Some(seq_id),
                        },
                        write_complete_tx: queued_message.write_complete_tx,
                    })
                    .await
                    .is_err()
                {
                    break;
                }
                seq_id = seq_id.wrapping_add(1);
            }
        }
    }

    let _ = writer_exited_tx.send(client_id).await;
}

pub(crate) async fn validate_remote_control_auth(auth_manager: &AuthManager) -> io::Result<()> {
    load_remote_control_auth(auth_manager).await.map(|_| ())
}

#[cfg(test)]
mod tests;
