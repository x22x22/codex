use crate::outgoing_message::ConnectionId;
use crate::outgoing_message::OutgoingMessage;
use crate::transport::AppServerTransport;
use crate::transport::CHANNEL_CAPACITY;
use crate::transport::ConnectionIdAllocator;
use crate::transport::TransportEvent;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCRequest;
use std::collections::HashMap;
use tokio::sync::mpsc;
use tokio::time::Duration;
use tokio::time::Instant;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

use super::ClientEvent;
use super::ClientId;
use super::PongStatus;
use super::ServerEvent;

const REMOTE_CONTROL_CLIENT_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const REMOTE_CONTROL_IDLE_SWEEP_INTERVAL: Duration = Duration::from_secs(30);

struct RemoteControlClientState {
    connection_id: ConnectionId,
    disconnect_token: CancellationToken,
    last_activity_at: Instant,
}

pub(super) async fn run(
    transport_event_tx: mpsc::Sender<TransportEvent>,
    mut client_event_rx: mpsc::Receiver<ClientEvent>,
    server_event_tx: mpsc::Sender<ServerEvent>,
    writer_exited_tx: mpsc::Sender<ClientId>,
    mut writer_exited_rx: mpsc::Receiver<ClientId>,
    shutdown_token: CancellationToken,
    connection_id_allocator: ConnectionIdAllocator,
) {
    let mut clients = HashMap::<ClientId, RemoteControlClientState>::new();
    let mut idle_sweep = tokio::time::interval(REMOTE_CONTROL_IDLE_SWEEP_INTERVAL);
    idle_sweep.set_missed_tick_behavior(MissedTickBehavior::Delay);

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
                match client_event {
                    ClientEvent::ClientMessage { client_id, message } => {
                        if let Some(connection_id) = clients.get_mut(&client_id).map(|client| {
                            client.last_activity_at = Instant::now();
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

                        if !remote_control_message_starts_connection(&message) {
                            continue;
                        }

                        let connection_id = connection_id_allocator.next_connection_id();
                        let (writer_tx, writer_rx) =
                            mpsc::channel::<OutgoingMessage>(CHANNEL_CAPACITY);
                        let disconnect_token = CancellationToken::new();
                        if transport_event_tx
                            .send(TransportEvent::ConnectionOpened {
                                connection_id,
                                transport_kind: AppServerTransport::RemoteControlled,
                                writer: writer_tx,
                                allow_legacy_notifications: false,
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
                    ClientEvent::Ping { client_id, .. } => {
                        let status = match clients.get_mut(&client_id) {
                            Some(client) => {
                                client.last_activity_at = Instant::now();
                                PongStatus::Active
                            }
                            None => PongStatus::Unknown,
                        };

                        if server_event_tx
                            .send(ServerEvent::Pong { client_id, status })
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    ClientEvent::ClientClosed { client_id } => {
                        if !close_remote_control_client(&transport_event_tx, &mut clients, &client_id)
                            .await
                        {
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
        JSONRPCMessage::Request(JSONRPCRequest { method, .. }) if method == "initialize"
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
            transport_kind: AppServerTransport::RemoteControlled,
        })
        .await
        .is_ok()
}

async fn run_remote_control_client_outbound(
    client_id: ClientId,
    mut writer_rx: mpsc::Receiver<OutgoingMessage>,
    server_event_tx: mpsc::Sender<ServerEvent>,
    writer_exited_tx: mpsc::Sender<ClientId>,
    disconnect_token: CancellationToken,
) {
    loop {
        tokio::select! {
            _ = disconnect_token.cancelled() => {
                break;
            }
            outgoing_message = writer_rx.recv() => {
                let Some(outgoing_message) = outgoing_message else {
                    break;
                };
                if server_event_tx
                    .send(ServerEvent::ServerMessage {
                        client_id: client_id.clone(),
                        message: Box::new(outgoing_message),
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }
    }

    let _ = writer_exited_tx.send(client_id).await;
}

#[cfg(test)]
#[path = "client_manager_tests.rs"]
mod client_manager_tests;
