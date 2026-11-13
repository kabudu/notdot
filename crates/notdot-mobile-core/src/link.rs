//! The phone side of pairing and the encrypted link, for native shells.
//!
//! The phone's identity key is created here and never leaves the process
//! through this API. (Persisting it in the Keychain / Android Keystore with
//! a this-device-only policy is the next step; the prototype keeps it in
//! memory, so a restart means pairing again.)
use crate::MobileError;
use notdot_link::{
    pairing::QrPayload,
    protocol::{Op, PairFrame, RequestId, ServerFrame},
    transport::{Link, NetConfig, Pairing, Phone, Route, desktop_addr},
};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum LinkRoute {
    /// Same network or a hole-punched path.
    Direct,
    /// Through the relay; still end-to-end encrypted.
    Relay,
    Unknown,
    Disconnected,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum PairResult {
    Approved { profile: String },
    Denied,
    Rejected { reason: String },
}

fn link_error(error: impl std::fmt::Display) -> MobileError {
    MobileError::Link {
        message: error.to_string(),
    }
}

struct State {
    phone: Option<Phone>,
    secret: Option<iroh_key::Secret>,
    qr: Option<QrPayload>,
    pairing: Option<Pairing>,
    link: Option<Link>,
    relay_only: bool,
}

/// Keeps the secret key private to this module.
mod iroh_key {
    pub struct Secret(pub(super) notdot_link::transport::SecretKey);
}

#[derive(uniffi::Object)]
pub struct PhoneLink {
    state: Arc<Mutex<State>>,
}

impl PhoneLink {
    async fn net(&self, state: &State, direct: bool) -> Result<NetConfig, MobileError> {
        let relays = match state.qr.as_ref().and_then(|qr| qr.relay.as_ref()) {
            Some(url) => Some(
                notdot_link::transport::relay_map(url, cfg!(debug_assertions))
                    .map_err(link_error)?,
            ),
            None => None,
        };
        Ok(NetConfig {
            relays,
            direct,
            insecure_test_relay: false,
            relay_roots: Vec::new(),
        })
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl PhoneLink {
    /// Debug builds also accept a plain-HTTP local development relay in
    /// the QR code; release builds require https.
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Arc::new(Mutex::new(State {
                phone: None,
                secret: None,
                qr: None,
                pairing: None,
                link: None,
                relay_only: false,
            })),
        })
    }

    /// Start pairing from a scanned (or typed) QR code. Returns the
    /// six-digit code to show; the desktop shows the same code.
    pub async fn pair(&self, qr: String, name: String) -> Result<String, MobileError> {
        let qr = QrPayload::decode(qr.trim()).map_err(link_error)?;
        let mut state = self.state.lock().await;
        state.qr = Some(qr.clone());
        let net = self.net(&state, true).await?;
        let phone = Phone::bind(state.secret.as_ref().map(|s| s.0.clone()), &net)
            .await
            .map_err(link_error)?;
        state.secret = Some(iroh_key::Secret(phone.secret()));
        let pairing = phone
            .pair(&qr, desktop_addr(&qr).map_err(link_error)?, &name, &[])
            .await
            .map_err(link_error)?;
        let sas = pairing.sas.to_string();
        state.pairing = Some(pairing);
        state.phone = Some(phone);
        Ok(sas)
    }

    /// Wait for the owner to approve or deny on the desktop.
    pub async fn await_approval(&self) -> Result<PairResult, MobileError> {
        let pairing = self
            .state
            .lock()
            .await
            .pairing
            .take()
            .ok_or_else(|| link_error("no pairing in progress"))?;
        Ok(match pairing.outcome().await.map_err(link_error)? {
            PairFrame::Approved { profile, .. } => PairResult::Approved { profile },
            PairFrame::Denied => PairResult::Denied,
            PairFrame::Rejected { reason } => PairResult::Rejected { reason },
            PairFrame::AwaitingApproval { .. } => PairResult::Rejected {
                reason: "unexpected reply".into(),
            },
        })
    }

    /// Connect to the paired desktop. `relay_only` drops direct paths, to
    /// prove the relay route (and to model a phone off the home network).
    pub async fn connect(&self, relay_only: bool) -> Result<LinkRoute, MobileError> {
        let mut state = self.state.lock().await;
        let qr = state.qr.clone().ok_or_else(|| link_error("not paired"))?;
        if relay_only != state.relay_only || state.phone.is_none() {
            if let Some(old) = state.phone.take() {
                old.close().await;
            }
            let net = self.net(&state, !relay_only).await?;
            let secret = state.secret.as_ref().map(|s| s.0.clone());
            state.phone = Some(Phone::bind(secret, &net).await.map_err(link_error)?);
            state.relay_only = relay_only;
        }
        let mut addr = desktop_addr(&qr).map_err(link_error)?;
        if relay_only {
            addr = notdot_link::transport::relay_only(&addr);
        }
        let phone = state.phone.as_ref().expect("bound above");
        let link = phone.connect(addr).await.map_err(link_error)?;
        let route = route(&link);
        state.link = Some(link);
        Ok(route)
    }

    pub async fn route(&self) -> LinkRoute {
        match &self.state.lock().await.link {
            Some(link) => route(link),
            None => LinkRoute::Disconnected,
        }
    }

    /// Add a note with an idempotency key the caller keeps for retries.
    pub async fn add_note(&self, request_id: String, text: String) -> Result<u64, MobileError> {
        let id = parse_id(&request_id)?;
        let reply = self.call(id, Op::AddNote { text }).await?;
        match reply {
            ServerFrame::Done { result, .. } => Ok(result["note"].as_u64().unwrap_or(0)),
            other => Err(link_error(format!("{other:?}"))),
        }
    }

    pub async fn list_notes(&self) -> Result<Vec<String>, MobileError> {
        match self.call(RequestId::random(), Op::ListNotes {}).await? {
            ServerFrame::Done { result, .. } => Ok(result["notes"]
                .as_array()
                .map(|notes| {
                    notes
                        .iter()
                        .filter_map(|n| n[1].as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default()),
            other => Err(link_error(format!("{other:?}"))),
        }
    }

    pub async fn disconnect(&self) {
        let mut state = self.state.lock().await;
        if let Some(link) = state.link.take() {
            link.close();
        }
    }
}

impl PhoneLink {
    async fn call(&self, id: RequestId, op: Op) -> Result<ServerFrame, MobileError> {
        let link = self
            .state
            .lock()
            .await
            .link
            .clone()
            .ok_or_else(|| link_error("not connected"))?;
        link.call(id, op, None).await.map_err(link_error)
    }
}

/// A fresh idempotency key (32 hex digits) for a new call.
#[uniffi::export]
pub fn new_request_id() -> String {
    format!("{:032x}", RequestId::random().0)
}

fn parse_id(text: &str) -> Result<RequestId, MobileError> {
    if text.len() != 32 {
        return Err(MobileError::Invalid {
            message: "request id must be 32 hex digits".into(),
        });
    }
    u128::from_str_radix(text, 16)
        .map(RequestId)
        .map_err(|_| MobileError::Invalid {
            message: "request id must be 32 hex digits".into(),
        })
}

fn route(link: &Link) -> LinkRoute {
    match link.route() {
        Route::Direct => LinkRoute::Direct,
        Route::Relay => LinkRoute::Relay,
        Route::Unknown => LinkRoute::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_ids_round_trip_and_bad_ones_are_refused() {
        let id = new_request_id();
        assert_eq!(format!("{:032x}", parse_id(&id).unwrap().0), id);
        assert!(parse_id("xyz").is_err());
        assert!(parse_id(&"g".repeat(32)).is_err());
    }
}
