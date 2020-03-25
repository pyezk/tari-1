// Copyright 2019, The Tari Project
//
// Redistribution and use in source and binary forms, with or without modification, are permitted provided that the
// following conditions are met:
//
// 1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following
// disclaimer.
//
// 2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the
// following disclaimer in the documentation and/or other materials provided with the distribution.
//
// 3. Neither the name of the copyright holder nor the names of its contributors may be used to endorse or promote
// products derived from this software without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES,
// INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
// DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
// SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
// SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY,
// WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE
// USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

use super::{
    dialer::{Dialer, DialerRequest},
    error::ConnectionManagerError,
    listener::PeerListener,
    peer_connection::{ConnId, PeerConnection},
    requester::ConnectionManagerRequest,
    types::ConnectionDirection,
};
use crate::{
    backoff::Backoff,
    noise::NoiseConfig,
    peer_manager::{NodeId, NodeIdentity},
    protocol::{ProtocolEvent, ProtocolId, Protocols},
    transports::Transport,
    types::DEFAULT_LISTENER_ADDRESS,
    PeerManager,
};
use futures::{
    channel::{mpsc, oneshot},
    stream::Fuse,
    AsyncRead,
    AsyncWrite,
    SinkExt,
    StreamExt,
};
use log::*;
use multiaddr::Multiaddr;
use std::{collections::HashMap, fmt, sync::Arc};
use tari_shutdown::{Shutdown, ShutdownSignal};
use time::Duration;
use tokio::{runtime, sync::broadcast, task, time};

const LOG_TARGET: &str = "comms::connection_manager::manager";

const EVENT_CHANNEL_SIZE: usize = 32;
const DIALER_REQUEST_CHANNEL_SIZE: usize = 32;

#[derive(Debug)]
pub enum ConnectionManagerEvent {
    // Peer connection
    PeerConnected(PeerConnection),
    PeerDisconnected(Box<NodeId>),
    PeerConnectFailed(Box<NodeId>, ConnectionManagerError),
    PeerConnectWillClose(ConnId, Box<NodeId>, ConnectionDirection),
    PeerInboundConnectFailed(ConnectionManagerError),

    // Listener
    Listening(Multiaddr),
    ListenFailed(ConnectionManagerError),

    // Substreams
    NewInboundSubstream(Box<NodeId>, ProtocolId, yamux::Stream),
}

impl fmt::Display for ConnectionManagerEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        use ConnectionManagerEvent::*;
        match self {
            PeerConnected(conn) => write!(f, "PeerConnected({})", conn),
            PeerDisconnected(node_id) => write!(f, "PeerDisconnected({})", node_id.short_str()),
            PeerConnectFailed(node_id, err) => write!(f, "PeerConnectFailed({}, {:?})", node_id.short_str(), err),
            PeerConnectWillClose(id, node_id, direction) => write!(
                f,
                "PeerConnectWillClose({}, {}, {})",
                id,
                node_id.short_str(),
                direction
            ),
            PeerInboundConnectFailed(err) => write!(f, "PeerInboundConnectFailed({:?})", err),
            Listening(addr) => write!(f, "Listening({})", addr),
            ListenFailed(err) => write!(f, "ListenFailed({:?})", err),
            NewInboundSubstream(node_id, protocol, _) => write!(
                f,
                "NewInboundSubstream({}, {}, Stream)",
                node_id.short_str(),
                String::from_utf8_lossy(protocol)
            ),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConnectionManagerConfig {
    /// The address to listen on for incoming connections. This address must be supported by the transport.
    /// Default: DEFAULT_LISTENER_ADDRESS constant
    pub listener_address: Multiaddr,
    /// The number of dial attempts to make before giving up. Default: 3
    pub max_dial_attempts: usize,
    /// The maximum number of connection tasks that will be spawned at the same time. Once this limit is reached, peers
    /// attempting to connect will have to wait for another connection attempt to complete. Default: 20
    pub max_simultaneous_inbound_connects: usize,
    /// The period of time to keep the peer connection around before disconnecting. Default: 3s
    pub disconnect_linger: Duration,
    /// Set to true to allow peers to send loopback, local-link and other addresses normally not considered valid for
    /// peer-to-peer comms. Default: false
    pub allow_test_addresses: bool,
}

impl Default for ConnectionManagerConfig {
    fn default() -> Self {
        Self {
            listener_address: DEFAULT_LISTENER_ADDRESS
                .parse()
                .expect("DEFAULT_LISTENER_ADDRESS is malformed"),
            max_dial_attempts: 3,
            max_simultaneous_inbound_connects: 20,
            disconnect_linger: Duration::from_secs(3),
            #[cfg(not(test))]
            allow_test_addresses: false,
            // This must always be true for internal crate tests
            #[cfg(test)]
            allow_test_addresses: true,
        }
    }
}

pub struct ConnectionManager<TTransport, TBackoff> {
    executor: runtime::Handle,
    config: ConnectionManagerConfig,
    request_rx: Fuse<mpsc::Receiver<ConnectionManagerRequest>>,
    internal_event_rx: Fuse<mpsc::Receiver<ConnectionManagerEvent>>,
    dialer_tx: mpsc::Sender<DialerRequest>,
    dialer: Option<Dialer<TTransport, TBackoff>>,
    listener: Option<PeerListener<TTransport>>,
    peer_manager: Arc<PeerManager>,
    node_identity: Arc<NodeIdentity>,
    active_connections: HashMap<NodeId, PeerConnection>,
    shutdown_signal: Option<ShutdownSignal>,
    protocols: Protocols<yamux::Stream>,
    listener_address: Option<Multiaddr>,
    listening_notifiers: Vec<oneshot::Sender<Multiaddr>>,
    connection_manager_events_tx: broadcast::Sender<Arc<ConnectionManagerEvent>>,
    complete_trigger: Shutdown,
}

impl<TTransport, TBackoff> ConnectionManager<TTransport, TBackoff>
where
    TTransport: Transport + Unpin + Send + Sync + Clone + 'static,
    TTransport::Output: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static,
    TBackoff: Backoff + Send + Sync + 'static,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: ConnectionManagerConfig,
        executor: runtime::Handle,
        transport: TTransport,
        noise_config: NoiseConfig,
        backoff: TBackoff,
        request_rx: mpsc::Receiver<ConnectionManagerRequest>,
        node_identity: Arc<NodeIdentity>,
        peer_manager: Arc<PeerManager>,
        protocols: Protocols<yamux::Stream>,
        connection_manager_events_tx: broadcast::Sender<Arc<ConnectionManagerEvent>>,
        shutdown_signal: ShutdownSignal,
    ) -> Self
    {
        let (internal_event_tx, internal_event_rx) = mpsc::channel(EVENT_CHANNEL_SIZE);

        let (dialer_tx, dialer_rx) = mpsc::channel(DIALER_REQUEST_CHANNEL_SIZE);

        let supported_protocols = protocols.get_supported_protocols();

        let listener = PeerListener::new(
            executor.clone(),
            config.clone(),
            transport.clone(),
            noise_config.clone(),
            internal_event_tx.clone(),
            peer_manager.clone(),
            Arc::clone(&node_identity),
            supported_protocols.clone(),
            shutdown_signal.clone(),
        );

        let dialer = Dialer::new(
            executor.clone(),
            config.clone(),
            Arc::clone(&node_identity),
            peer_manager.clone(),
            transport,
            noise_config,
            backoff,
            dialer_rx,
            internal_event_tx,
            supported_protocols,
            shutdown_signal.clone(),
        );

        Self {
            executor,
            config,
            shutdown_signal: Some(shutdown_signal),
            request_rx: request_rx.fuse(),
            node_identity,
            peer_manager,
            protocols,
            internal_event_rx: internal_event_rx.fuse(),
            dialer_tx,
            dialer: Some(dialer),
            listener: Some(listener),
            active_connections: Default::default(),
            listener_address: None,
            listening_notifiers: Vec::new(),
            connection_manager_events_tx,
            complete_trigger: Shutdown::new(),
        }
    }

    pub fn complete_signal(&self) -> ShutdownSignal {
        self.complete_trigger.to_signal()
    }

    pub async fn run(mut self) {
        let mut shutdown = self
            .shutdown_signal
            .take()
            .expect("ConnectionManager initialized without a shutdown");

        self.run_listener();
        self.run_dialer();

        debug!(target: LOG_TARGET, "Connection manager started");
        loop {
            futures::select! {
                event = self.internal_event_rx.select_next_some() => {
                    self.handle_event(event).await;
                },

                request = self.request_rx.select_next_some() => {
                    self.handle_request(request).await;
                },

                _ = shutdown => {
                    info!(target: LOG_TARGET, "ConnectionManager is shutting down because it received the shutdown signal");
                    self.disconnect_all().await;
                    break;
                }
            }
        }
    }

    async fn disconnect_all(&mut self) {
        let mut node_ids = Vec::with_capacity(self.active_connections.len());
        for (node_id, mut conn) in self.active_connections.drain() {
            if !conn.is_connected() {
                continue;
            }

            match conn.disconnect_silent().await {
                Ok(_) => {
                    node_ids.push(node_id);
                },
                Err(err) => {
                    error!(
                        target: LOG_TARGET,
                        "In disconnect_all: Error when disconnecting peer '{}' because '{:?}'",
                        node_id.short_str(),
                        err
                    );
                },
            }
        }

        for node_id in node_ids {
            self.publish_event(ConnectionManagerEvent::PeerDisconnected(Box::new(node_id)));
        }
    }

    fn run_listener(&mut self) {
        let listener = self
            .listener
            .take()
            .expect("ConnnectionManager initialized without a Listener");

        self.executor.spawn(listener.run());
    }

    fn run_dialer(&mut self) {
        let dialer = self
            .dialer
            .take()
            .expect("ConnnectionManager initialized without an Establisher");

        self.executor.spawn(dialer.run());
    }

    async fn handle_request(&mut self, request: ConnectionManagerRequest) {
        use ConnectionManagerRequest::*;
        trace!(target: LOG_TARGET, "Connection manager got request: {:?}", request);
        match request {
            DialPeer(node_id, is_forced, reply_tx) => match self.get_active_connection(&node_id) {
                Some(conn) => {
                    debug!(target: LOG_TARGET, "[{}] Found existing active connection", conn);
                    log_if_error_fmt!(
                        target: LOG_TARGET,
                        reply_tx.send(Ok(conn.clone())),
                        "Failed to send reply for dial request for peer '{}'",
                        node_id.short_str()
                    );
                },
                None => {
                    debug!(
                        target: LOG_TARGET,
                        "[ThisNode={}] Existing peer connection NOT found. Attempting to establish a new connection \
                         to peer '{}'.",
                        self.node_identity.node_id().short_str(),
                        node_id.short_str()
                    );
                    self.dial_peer(node_id, reply_tx, is_forced).await
                },
            },
            NotifyListening(reply_tx) => match self.listener_address.as_ref() {
                Some(addr) => {
                    let _ = reply_tx.send(addr.clone());
                },
                None => {
                    self.listening_notifiers.push(reply_tx);
                },
            },
            GetActiveConnection(node_id, reply_tx) => {
                let _ = reply_tx.send(self.active_connections.get(&node_id).map(Clone::clone));
            },
            GetActiveConnections(reply_tx) => {
                let _ = reply_tx.send(self.active_connections.values().cloned().collect());
            },
            DisconnectPeer(node_id, reply_tx) => match self.active_connections.remove(&node_id) {
                Some(mut conn) => {
                    let _ = reply_tx.send(conn.disconnect().await.map_err(Into::into));
                },
                None => {
                    let _ = reply_tx.send(Ok(()));
                },
            },
        }
    }

    async fn handle_event(&mut self, event: ConnectionManagerEvent) {
        use ConnectionManagerEvent::*;

        trace!(
            target: LOG_TARGET,
            "[ThisNode = {}] Received internal event '{}'",
            self.node_identity.node_id().short_str(),
            event
        );

        match event {
            Listening(addr) => {
                self.listener_address = Some(addr.clone());
                self.publish_event(ConnectionManagerEvent::Listening(addr.clone()));
                for notifier in self.listening_notifiers.drain(..) {
                    let _ = notifier.send(addr.clone());
                }
            },
            NewInboundSubstream(node_id, protocol, stream) => {
                let proto_str = String::from_utf8_lossy(&protocol);
                debug!(
                    target: LOG_TARGET,
                    "New inbound substream for peer '{}' speaking protocol '{}'",
                    node_id.short_str(),
                    proto_str
                );
                if let Err(err) = self
                    .protocols
                    .notify(&protocol, ProtocolEvent::NewInboundSubstream(node_id, stream))
                    .await
                {
                    error!(
                        target: LOG_TARGET,
                        "Error sending NewSubstream notification for protocol '{}' because '{:?}'", proto_str, err
                    );
                }
            },
            PeerConnected(new_conn) => {
                let node_id = new_conn.peer_node_id().clone();

                if let Err(err) = self.peer_manager.set_last_connect_success(&node_id).await {
                    error!(
                        target: LOG_TARGET,
                        "set_last_connect_success failed because '{:?}'", err
                    );
                }

                // If we're dialing this node, let's cancel it
                self.send_dialer_request(DialerRequest::CancelPendingDial(node_id.clone()))
                    .await;

                match self.active_connections.remove(&node_id) {
                    Some(existing_conn) => {
                        debug!(
                            target: LOG_TARGET,
                            "Existing {} peer connection found for peer '{}'",
                            existing_conn.direction(),
                            existing_conn.peer_node_id()
                        );

                        if self.tie_break_existing_connection(&existing_conn, &new_conn) {
                            debug!(
                                target: LOG_TARGET,
                                "Disconnecting existing {} connection to peer '{}' because of simultaneous dial",
                                existing_conn.direction(),
                                existing_conn.peer_node_id().short_str()
                            );

                            self.publish_event(PeerConnectWillClose(
                                existing_conn.id(),
                                Box::new(existing_conn.peer_node_id().clone()),
                                existing_conn.direction(),
                            ));
                            self.delayed_disconnect(existing_conn);
                            self.active_connections.insert(node_id, new_conn.clone());
                            self.publish_event(PeerConnected(new_conn));
                        } else {
                            debug!(
                                target: LOG_TARGET,
                                "Disconnecting new {} connection to peer '{}' because of
                         simultaneous dial",
                                new_conn.direction(),
                                new_conn.peer_node_id().short_str()
                            );

                            self.delayed_disconnect(new_conn);
                            self.active_connections.insert(node_id, existing_conn);
                        }
                    },
                    None => {
                        debug!(
                            target: LOG_TARGET,
                            "Adding new {} peer connection for peer '{}'",
                            new_conn.direction(),
                            new_conn.peer_node_id().short_str()
                        );
                        self.active_connections.insert(node_id, new_conn.clone());
                        self.publish_event(PeerConnected(new_conn));
                    },
                }
            },
            PeerDisconnected(node_id) => {
                if self.active_connections.remove(&node_id).is_some() {
                    self.publish_event(PeerDisconnected(node_id));
                }
            },
            PeerConnectFailed(node_id, err) => {
                if let Err(err) = self.peer_manager.set_last_connect_failed(&node_id).await {
                    error!(target: LOG_TARGET, "set_peer_connect_failed failed because '{:?}'", err);
                }
                self.publish_event(PeerConnectFailed(node_id, err));
            },
            event => {
                self.publish_event(event);
            },
        }

        trace!(
            target: LOG_TARGET,
            "[ThisNode={}] {} active connection(s)",
            self.node_identity.node_id().short_str(),
            self.active_connections.len()
        );
    }

    #[inline]
    async fn send_dialer_request(&mut self, req: DialerRequest) {
        if let Err(err) = self.dialer_tx.send(req).await {
            error!(target: LOG_TARGET, "Unable to send DialerRequest because '{}'", err);
        }
    }

    /// Two connections to the same peer have been created. This function deterministically determines which peer
    /// connection to close. It does this by comparing our NodeId to that of the peer. This rule enables both sides to
    /// agree which connection to disconnect
    ///
    /// Returns true if the existing connection should close, otherwise false if the new connection should be closed.
    fn tie_break_existing_connection(&self, existing_conn: &PeerConnection, new_conn: &PeerConnection) -> bool {
        debug_assert_eq!(existing_conn.peer_node_id(), new_conn.peer_node_id());
        let peer_node_id = existing_conn.peer_node_id();
        let our_node_id = self.node_identity.node_id();

        use ConnectionDirection::*;
        match (existing_conn.direction(), new_conn.direction()) {
            // They connected to us twice for some reason. Drop the existing (older) connection
            (Inbound, Inbound) => true,
            // They connected to us at the same time we connected to them
            (Inbound, Outbound) => peer_node_id > our_node_id,
            // We connected to them at the same time as they connected to us
            (Outbound, Inbound) => our_node_id > peer_node_id,
            // We connected to them twice for some reason. Drop the newer connection.
            (Outbound, Outbound) => false,
        }
    }

    /// A 'gentle' disconnect starts by firing a `PeerConnectWillClose` event, waiting (lingering) for a period of time
    /// and then disconnecting. This gives other components time to conclude their work before the connection is
    /// closed.
    fn delayed_disconnect(&mut self, mut conn: PeerConnection) -> task::JoinHandle<()> {
        let linger = self.config.disconnect_linger;
        debug!(
            target: LOG_TARGET,
            "{} connection for peer '{}' will close after {}ms",
            conn.direction(),
            conn.peer_node_id(),
            linger.as_millis()
        );

        self.executor.spawn(async move {
            debug!(
                target: LOG_TARGET,
                "Waiting for linger period ({}ms) to expire...",
                linger.as_millis()
            );
            time::delay_for(linger).await;
            if conn.is_connected() {
                match conn.disconnect_silent().await {
                    Ok(_) => {},
                    Err(err) => {
                        error!(target: LOG_TARGET, "Failed to disconnect because '{:?}'", err);
                    },
                }
            }
        })
    }

    fn publish_event(&self, event: ConnectionManagerEvent) {
        let event = Arc::new(event);
        if self.connection_manager_events_tx.send(event.clone()).is_err() {
            trace!(
                target: LOG_TARGET,
                "Didn't send event '{}' because there are no subscribers",
                event
            );
        }
    }

    #[inline]
    fn get_active_connection(&self, node_id: &NodeId) -> Option<&PeerConnection> {
        self.active_connections.get(node_id)
    }

    async fn dial_peer(
        &mut self,
        node_id: NodeId,
        reply_tx: oneshot::Sender<Result<PeerConnection, ConnectionManagerError>>,
        force_dial: bool,
    )
    {
        match self.peer_manager.find_by_node_id(&node_id).await {
            Ok(peer) => {
                if !force_dial && peer.is_recently_offline() {
                    debug!(
                        target: LOG_TARGET,
                        "Peer '{}' is offline (i.e. we failed to connect to them recently).",
                        peer.node_id.short_str()
                    );
                    let _ = reply_tx.send(Err(ConnectionManagerError::PeerOffline));
                    self.publish_event(ConnectionManagerEvent::PeerConnectFailed(
                        Box::new(peer.node_id),
                        ConnectionManagerError::PeerOffline,
                    ));
                    return;
                }

                if let Err(err) = self.dialer_tx.try_send(DialerRequest::Dial(Box::new(peer), reply_tx)) {
                    error!(target: LOG_TARGET, "Failed to send request to dialer because '{}'", err);
                    // TODO: If the channel is full - we'll fail to dial. This function should block until the dial
                    //       request channel has cleared

                    if let DialerRequest::Dial(_, reply_tx) = err.into_inner() {
                        log_if_error_fmt!(
                            target: LOG_TARGET,
                            reply_tx.send(Err(ConnectionManagerError::EstablisherChannelError)),
                            "Failed to send dial peer result for peer '{}'",
                            node_id.short_str()
                        );
                    }
                }
            },
            Err(err) => {
                error!(target: LOG_TARGET, "Failed to fetch peer to dial because '{}'", err);
                log_if_error_fmt!(
                    level: warn,
                    target: LOG_TARGET,
                    reply_tx.send(Err(ConnectionManagerError::PeerManagerError(err))),
                    "Failed to send error reply when dialing peer '{}'",
                    node_id.short_str()
                );
            },
        }
    }
}
