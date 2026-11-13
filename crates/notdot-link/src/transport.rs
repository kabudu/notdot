//! Iroh QUIC endpoints for the desktop and the phone.
//!
//! Each endpoint is its Ed25519 key; QUIC/TLS 1.3 authenticates both keys on
//! every connection, so the phone pins the desktop by dialling its key, and
//! the desktop identifies the phone by the key it authenticated with. A relay
//! only forwards encrypted packets: it can see that two keys talk, and when,
//! but not what they say. Paths (LAN, hole-punched, relayed) can change under
//! a live connection; calls are idempotent for when one cannot.
//!
//! This prototype never uses third-party discovery or relays: callers pass a
//! relay map explicitly (tests run a local relay).
use crate::{
    DeviceId, LinkError,
    pairing::{
        DeviceProfile, DeviceRecord, DeviceRegistry, OfferState, PairRequest, PairingDesk,
        PendingApproval, ProfileRef, QrPayload, Refusal, Sas,
    },
    protocol::{
        ClientFrame, LINK_ALPN, LINK_VERSION, Op, OpError, PAIR_ALPN, PairFrame, RequestId,
        ServerFrame, StepUpProof, frame,
    },
    service::{Clock, Service},
};
use iroh::{
    Endpoint, EndpointAddr, PublicKey, RelayMap, RelayMode, TransportAddr,
    endpoint::{Connection, ConnectionError, QuicTransportConfig, VarInt, presets},
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::watch;

/// Concurrent request streams one phone may open; more wait in QUIC flow
/// control, which is the transport-level backpressure.
pub const MAX_STREAMS: u32 = 16;
/// A pairing connection waits this long at most for the owner to decide.
pub const APPROVAL_WAIT: Duration = Duration::from_secs(120);
const CLOSE_OK: u32 = 0;

fn refusal_code(refusal: Refusal) -> u32 {
    match refusal {
        Refusal::Unknown => 1,
        Refusal::Revoked => 2,
        Refusal::StaleProfile => 3,
    }
}
fn refusal_from_code(code: u64) -> Option<Refusal> {
    match code {
        1 => Some(Refusal::Unknown),
        2 => Some(Refusal::Revoked),
        3 => Some(Refusal::StaleProfile),
        _ => None,
    }
}

/// How an endpoint may reach peers.
#[derive(Clone, Debug)]
pub struct NetConfig {
    /// Relays this endpoint registers with and may use.
    pub relays: Option<RelayMap>,
    /// Use direct IP paths (LAN, hole punching). `false` forces the relay.
    pub direct: bool,
    /// Accept the local test relay's self-signed certificate. Tests only.
    pub insecure_test_relay: bool,
    /// Trust only these DER root certificates for relay TLS (a private CA
    /// or a local proxy). Empty: the embedded public web roots.
    pub relay_roots: Vec<Vec<u8>>,
}

async fn bind(
    secret: Option<SecretKey>,
    net: &NetConfig,
    alpns: Vec<Vec<u8>>,
) -> Result<Endpoint, LinkError> {
    let transport = QuicTransportConfig::builder()
        .max_concurrent_bidi_streams(VarInt::from_u32(MAX_STREAMS))
        .max_concurrent_uni_streams(VarInt::from_u32(0))
        .build();
    let mut builder = Endpoint::builder(presets::Minimal)
        .relay_mode(match &net.relays {
            Some(map) => RelayMode::Custom(map.clone()),
            None => RelayMode::Disabled,
        })
        .transport_config(transport)
        .alpns(alpns);
    if let Some(secret) = secret {
        builder = builder.secret_key(secret);
    }
    if !net.direct {
        builder = builder.clear_ip_transports();
    }
    if !net.relay_roots.is_empty() {
        builder = builder.ca_tls_config(iroh::tls::CaTlsConfig::custom_roots(
            net.relay_roots
                .iter()
                .map(|der| rustls_pki_types::CertificateDer::from(der.clone())),
        ));
    }
    if net.insecure_test_relay {
        #[cfg(feature = "test-relay")]
        {
            builder = builder.ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify());
        }
        #[cfg(not(feature = "test-relay"))]
        return Err(LinkError::Connection(
            "test relay support is not compiled in".into(),
        ));
    }
    builder
        .bind()
        .await
        .map_err(|e| LinkError::Connection(e.to_string()))
}

fn device_id(key: PublicKey) -> DeviceId {
    DeviceId(*key.as_bytes())
}

fn now_ms(clock: &Clock) -> u64 {
    clock()
}

struct DesktopState {
    desk: Mutex<PairingDesk>,
    registry: Mutex<DeviceRegistry>,
    epochs: Mutex<HashMap<String, u64>>,
    live: Mutex<HashMap<DeviceId, Vec<Connection>>>,
    changed: watch::Sender<u64>,
    service: Service,
    clock: Clock,
}
impl DesktopState {
    fn notify(&self) {
        self.changed.send_modify(|generation| *generation += 1);
    }
    fn authorize(&self, peer: DeviceId) -> Result<DeviceRecord, Refusal> {
        let epochs = self.epochs.lock().expect("epochs lock").clone();
        self.registry
            .lock()
            .expect("registry lock")
            .authorize(peer, |profile| epochs.get(profile).copied())
            .cloned()
    }
}

/// The desktop side: accepts pairing and paired-device connections only.
pub struct Desktop {
    endpoint: Endpoint,
    state: Arc<DesktopState>,
    accept: tokio::task::JoinHandle<()>,
}
impl Desktop {
    pub async fn bind(
        secret: Option<SecretKey>,
        net: &NetConfig,
        clock: Clock,
    ) -> Result<Self, LinkError> {
        let endpoint = bind(secret, net, vec![PAIR_ALPN.to_vec(), LINK_ALPN.to_vec()]).await?;
        let id = device_id(endpoint.id());
        let state = Arc::new(DesktopState {
            desk: Mutex::new(PairingDesk::new(id)),
            registry: Mutex::new(DeviceRegistry::default()),
            epochs: Mutex::new(HashMap::new()),
            live: Mutex::new(HashMap::new()),
            changed: watch::channel(0).0,
            service: Service::new(id, clock.clone()),
            clock,
        });
        let accept = tokio::spawn(accept_loop(endpoint.clone(), state.clone()));
        Ok(Self {
            endpoint,
            state,
            accept,
        })
    }

    pub fn id(&self) -> DeviceId {
        device_id(self.endpoint.id())
    }

    /// Wait until the endpoint has a relay connection (if any relay).
    pub async fn online(&self) {
        self.endpoint.online().await;
    }

    pub fn addr(&self) -> EndpointAddr {
        self.endpoint.addr()
    }

    pub fn service(&self) -> &Service {
        &self.state.service
    }

    pub fn set_profile_epoch(&self, profile: &str, epoch: u64) {
        self.state
            .epochs
            .lock()
            .expect("epochs lock")
            .insert(profile.to_owned(), epoch);
        self.state.notify();
    }

    /// Owner action: show a QR for one phone. Network hints are added from
    /// the endpoint's current addresses.
    pub fn create_offer(
        &self,
        profile: ProfileRef,
        device_profile: DeviceProfile,
        ttl_ms: u64,
    ) -> Result<QrPayload, LinkError> {
        let now = now_ms(&self.state.clock);
        let mut qr = self.state.desk.lock().expect("desk lock").create_offer(
            profile,
            device_profile,
            now,
            ttl_ms,
        )?;
        let addr = self.endpoint.addr();
        qr.relay = addr.relay_urls().next().map(ToString::to_string);
        qr.direct = addr.ip_addrs().copied().take(8).collect();
        Ok(qr)
    }

    pub fn pending(&self) -> Vec<PendingApproval> {
        self.state
            .desk
            .lock()
            .expect("desk lock")
            .pending()
            .cloned()
            .collect()
    }

    /// Owner approves after comparing codes.
    pub fn approve(&self, pending: &PendingApproval) -> Result<DeviceRecord, LinkError> {
        let now = now_ms(&self.state.clock);
        let record = {
            let mut desk = self.state.desk.lock().expect("desk lock");
            let mut registry = self.state.registry.lock().expect("registry lock");
            desk.approve(&pending.offer, &mut registry, now)?
        };
        self.state.notify();
        Ok(record)
    }

    pub fn deny(&self, pending: &PendingApproval) {
        self.state
            .desk
            .lock()
            .expect("desk lock")
            .deny(&pending.offer);
        self.state.notify();
    }

    /// Remove a phone and drop its live connections at once.
    pub fn revoke(&self, device: DeviceId) {
        // Hold the live lock across the registry change: serve_link
        // registers and re-checks under the same lock, so no connection
        // can slip between revocation and the close below.
        let mut live = self.state.live.lock().expect("live lock");
        self.state
            .registry
            .lock()
            .expect("registry lock")
            .revoke(device);
        let connections = live.remove(&device).unwrap_or_default();
        drop(live);
        for connection in connections {
            connection.close(VarInt::from_u32(refusal_code(Refusal::Revoked)), b"revoked");
        }
    }

    pub fn grant_sensitive(&self, device: DeviceId, until_ms: u64) -> Result<(), LinkError> {
        self.state
            .registry
            .lock()
            .expect("registry lock")
            .grant_sensitive(device, until_ms)
    }

    pub async fn close(self) {
        self.accept.abort();
        self.endpoint.close().await;
    }
}

async fn accept_loop(endpoint: Endpoint, state: Arc<DesktopState>) {
    while let Some(incoming) = endpoint.accept().await {
        let state = state.clone();
        tokio::spawn(async move {
            let Ok(connection) = incoming.await else {
                return;
            };
            let peer = device_id(connection.remote_id());
            match connection.alpn() {
                alpn if alpn == PAIR_ALPN => serve_pairing(connection, peer, state).await,
                alpn if alpn == LINK_ALPN => serve_link(connection, peer, state).await,
                _ => connection.close(VarInt::from_u32(CLOSE_OK), b"unknown protocol"),
            }
        });
    }
}

async fn serve_pairing(connection: Connection, peer: DeviceId, state: Arc<DesktopState>) {
    let Ok((mut send, mut recv)) = connection.accept_bi().await else {
        return;
    };
    let reply = async {
        let request: PairRequest = frame::read(&mut recv)
            .await?
            .ok_or(LinkError::BadRequest("empty pairing stream"))?;
        let now = now_ms(&state.clock);
        let pending = state
            .desk
            .lock()
            .expect("desk lock")
            .submit(peer, &request, now)?;
        state.notify();
        frame::write(&mut send, &PairFrame::AwaitingApproval { sas: pending.sas }).await?;
        let mut changed = state.changed.subscribe();
        let deadline = tokio::time::Instant::now() + APPROVAL_WAIT;
        loop {
            let decision = state
                .desk
                .lock()
                .expect("desk lock")
                .state(&pending.offer)
                .cloned();
            match decision {
                Some(OfferState::Approved(device)) if device == peer => {
                    let record = state
                        .registry
                        .lock()
                        .expect("registry lock")
                        .get(&peer)
                        .cloned()
                        .ok_or(LinkError::Denied)?;
                    return Ok(PairFrame::Approved {
                        profile: record.profile.id,
                        scopes: record.scopes.into_iter().collect(),
                    });
                }
                Some(OfferState::AwaitingApproval(_)) => {}
                Some(OfferState::Denied) => return Ok(PairFrame::Denied),
                _ => return Err(LinkError::OfferUnavailable),
            }
            if tokio::time::timeout_at(deadline, changed.changed())
                .await
                .is_err()
            {
                return Err(LinkError::OfferUnavailable);
            }
        }
    }
    .await;
    let reply = reply.unwrap_or_else(|error| PairFrame::Rejected {
        reason: error.to_string(),
    });
    if frame::write(&mut send, &reply).await.is_ok() && send.finish().is_ok() {
        // Let the phone read the outcome before the connection goes away.
        let _ = tokio::time::timeout(Duration::from_secs(5), connection.closed()).await;
    }
}

async fn serve_link(connection: Connection, peer: DeviceId, state: Arc<DesktopState>) {
    if let Err(refusal) = state.authorize(peer) {
        connection.close(
            VarInt::from_u32(refusal_code(refusal)),
            refusal.to_string().as_bytes(),
        );
        return;
    }
    {
        // Register, then re-check under the same lock `revoke` takes, so a
        // revocation between the first check and here still closes us.
        let mut live = state.live.lock().expect("live lock");
        if let Err(refusal) = state.authorize(peer) {
            drop(live);
            connection.close(
                VarInt::from_u32(refusal_code(refusal)),
                refusal.to_string().as_bytes(),
            );
            return;
        }
        live.entry(peer).or_default().push(connection.clone());
    }
    while let Ok((mut send, mut recv)) = connection.accept_bi().await {
        let state = state.clone();
        let connection = connection.clone();
        tokio::spawn(async move {
            let Ok(Some(request)) = frame::read::<ClientFrame>(&mut recv).await else {
                return;
            };
            // Re-check on every call: revocation and epochs apply at once.
            let record = match state.authorize(peer) {
                Ok(record) => record,
                Err(refusal) => {
                    connection.close(
                        VarInt::from_u32(refusal_code(refusal)),
                        refusal.to_string().as_bytes(),
                    );
                    return;
                }
            };
            let reply = match request {
                ClientFrame::Hello { versions } if versions.contains(&LINK_VERSION) => {
                    ServerFrame::Welcome {
                        version: LINK_VERSION,
                    }
                }
                ClientFrame::Hello { .. } => ServerFrame::Failed {
                    id: RequestId(0),
                    error: OpError::UnsupportedVersion {
                        supported: vec![LINK_VERSION],
                    },
                },
                ClientFrame::Call { id, op, step_up } => {
                    state.service.call(&record, id, op, step_up).await
                }
                ClientFrame::Cancel { id } => ServerFrame::CancelAck {
                    id,
                    found: state.service.cancel(&peer, id),
                },
            };
            let written = match frame::write(&mut send, &reply).await {
                // Never drop a call silently: say why instead.
                Err(LinkError::FrameTooLarge) => {
                    let refusal = ServerFrame::Failed {
                        id: reply.id().unwrap_or(RequestId(0)),
                        error: OpError::BadRequest {
                            reason: "reply too large".into(),
                        },
                    };
                    frame::write(&mut send, &refusal).await
                }
                other => other,
            };
            if written.is_ok() {
                let _ = send.finish();
            }
        });
    }
    if let Some(list) = state.live.lock().expect("live lock").get_mut(&peer) {
        list.retain(|c| c.stable_id() != connection.stable_id());
    }
}

/// Where a connection's packets currently go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// Directly over IP (same network, or hole-punched).
    Direct,
    /// Through the relay.
    Relay,
    Unknown,
}

/// The phone side.
pub struct Phone {
    endpoint: Endpoint,
}

/// A pairing in progress: show `sas`, then await the desktop's decision.
pub struct Pairing {
    pub sas: Sas,
    connection: Connection,
    recv: iroh::endpoint::RecvStream,
}
impl Pairing {
    pub async fn outcome(mut self) -> Result<PairFrame, LinkError> {
        let outcome = frame::read::<PairFrame>(&mut self.recv)
            .await?
            .ok_or(LinkError::Connection("pairing ended".into()))?;
        self.connection.close(VarInt::from_u32(CLOSE_OK), b"done");
        Ok(outcome)
    }
}

pub use iroh::SecretKey;

/// A relay map holding one relay URL (as carried in a QR code). Plain
/// `http` relays (a local development relay) are refused unless allowed.
pub fn relay_map(url: &str, allow_plain_http: bool) -> Result<RelayMap, LinkError> {
    let url: iroh::RelayUrl = url.parse().map_err(|_| LinkError::BadQr("relay url"))?;
    match url.scheme() {
        "https" => {}
        "http" if allow_plain_http => {}
        _ => return Err(LinkError::BadQr("relay must use https")),
    }
    Ok(RelayMap::from(url))
}

/// The same address with only its relay paths, to force the relay route.
pub fn relay_only(addr: &EndpointAddr) -> EndpointAddr {
    EndpointAddr::from_parts(
        addr.id,
        addr.addrs
            .iter()
            .filter(|a| matches!(a, TransportAddr::Relay(_)))
            .cloned(),
    )
}

/// Build the desktop's address from a scanned QR. The key is what is
/// trusted; the hints only help find it.
pub fn desktop_addr(qr: &QrPayload) -> Result<EndpointAddr, LinkError> {
    let key = PublicKey::from_bytes(&qr.desktop.0).map_err(|_| LinkError::BadQr("desktop key"))?;
    let mut addrs: Vec<TransportAddr> = qr.direct.iter().copied().map(TransportAddr::Ip).collect();
    if let Some(relay) = &qr.relay {
        addrs.push(TransportAddr::Relay(
            relay.parse().map_err(|_| LinkError::BadQr("relay url"))?,
        ));
    }
    Ok(EndpointAddr::from_parts(key, addrs))
}

impl Phone {
    pub async fn bind(secret: Option<SecretKey>, net: &NetConfig) -> Result<Self, LinkError> {
        Ok(Self {
            endpoint: bind(secret, net, Vec::new()).await?,
        })
    }

    pub fn id(&self) -> DeviceId {
        device_id(self.endpoint.id())
    }

    pub async fn online(&self) {
        self.endpoint.online().await;
    }

    /// The phone's identity key, for its platform keystore. The prototype
    /// keeps it in memory; it never leaves the phone.
    pub fn secret(&self) -> SecretKey {
        self.endpoint.secret_key().clone()
    }

    /// Start pairing with a scanned QR. `addr` is normally
    /// [`desktop_addr`]`(qr)`; its key must be the QR's desktop key.
    pub async fn pair(
        &self,
        qr: &QrPayload,
        addr: EndpointAddr,
        name: &str,
        step_up_key: &[u8],
    ) -> Result<Pairing, LinkError> {
        if device_id(addr.id) != qr.desktop {
            return Err(LinkError::BadQr("address is not the QR's desktop"));
        }
        let (request, sas) = crate::pairing::phone_request(qr, self.id(), name, step_up_key);
        let connection = self
            .endpoint
            .connect(addr, PAIR_ALPN)
            .await
            .map_err(|e| LinkError::Connection(e.to_string()))?;
        let (mut send, mut recv) = connection
            .open_bi()
            .await
            .map_err(|e| LinkError::Connection(e.to_string()))?;
        frame::write(&mut send, &request).await?;
        send.finish().map_err(LinkError::io)?;
        match frame::read::<PairFrame>(&mut recv).await? {
            Some(PairFrame::AwaitingApproval { sas: desktop_sas }) if desktop_sas == sas => {
                Ok(Pairing {
                    sas,
                    connection,
                    recv,
                })
            }
            Some(PairFrame::AwaitingApproval { .. }) => {
                Err(LinkError::Connection("comparison codes differ".into()))
            }
            Some(PairFrame::Rejected { reason }) => Err(LinkError::Connection(reason)),
            other => Err(LinkError::Connection(format!("unexpected {other:?}"))),
        }
    }

    pub async fn connect(&self, addr: EndpointAddr) -> Result<Link, LinkError> {
        let connection = self
            .endpoint
            .connect(addr, LINK_ALPN)
            .await
            .map_err(|e| LinkError::Connection(e.to_string()))?;
        let link = Link { connection };
        match link
            .request(&ClientFrame::Hello {
                versions: vec![LINK_VERSION],
            })
            .await?
        {
            ServerFrame::Welcome { .. } => Ok(link),
            other => Err(LinkError::Connection(format!("handshake: {other:?}"))),
        }
    }

    pub async fn close(self) {
        self.endpoint.close().await;
    }
}

/// An authorised connection to the desktop.
#[derive(Clone)]
pub struct Link {
    connection: Connection,
}
impl Link {
    fn error(&self, error: impl std::fmt::Display) -> LinkError {
        if let Some(ConnectionError::ApplicationClosed(close)) = self.connection.close_reason()
            && let Some(refusal) = refusal_from_code(close.error_code.into_inner())
        {
            return LinkError::Refused(refusal);
        }
        LinkError::Connection(error.to_string())
    }

    async fn request(&self, request: &ClientFrame) -> Result<ServerFrame, LinkError> {
        let (mut send, mut recv) = self.connection.open_bi().await.map_err(|e| self.error(e))?;
        frame::write(&mut send, request)
            .await
            .map_err(|e| self.error(e))?;
        send.finish().map_err(|e| self.error(e))?;
        frame::read::<ServerFrame>(&mut recv)
            .await
            .map_err(|e| self.error(e))?
            .ok_or_else(|| self.error("stream ended without a reply"))
    }

    pub async fn call(
        &self,
        id: RequestId,
        op: Op,
        step_up: Option<StepUpProof>,
    ) -> Result<ServerFrame, LinkError> {
        self.request(&ClientFrame::Call { id, op, step_up }).await
    }

    pub async fn cancel(&self, id: RequestId) -> Result<bool, LinkError> {
        match self.request(&ClientFrame::Cancel { id }).await? {
            ServerFrame::CancelAck { found, .. } => Ok(found),
            other => Err(LinkError::Connection(format!("unexpected {other:?}"))),
        }
    }

    /// The route currently carrying traffic.
    pub fn route(&self) -> Route {
        self.connection
            .paths()
            .iter()
            .find(|path| path.is_selected())
            .map_or(Route::Unknown, |path| match path.remote_addr() {
                TransportAddr::Relay(_) => Route::Relay,
                TransportAddr::Ip(_) => Route::Direct,
                _ => Route::Unknown,
            })
    }

    /// Wait for the desktop to close the connection; returns its refusal.
    pub async fn closed(&self) -> Option<Refusal> {
        match self.connection.closed().await {
            ConnectionError::ApplicationClosed(close) => {
                refusal_from_code(close.error_code.into_inner())
            }
            _ => None,
        }
    }

    pub fn close(&self) {
        self.connection.close(VarInt::from_u32(CLOSE_OK), b"bye");
    }
}
