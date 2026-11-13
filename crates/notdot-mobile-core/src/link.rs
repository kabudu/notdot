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
        detail: error.to_string(),
    }
}

struct State {
    secret: Option<iroh_key::Secret>,
    /// The desktop this phone is paired with; set only after approval.
    paired: Option<QrPayload>,
    /// A pairing waiting for the owner, with its endpoint.
    pending: Option<(QrPayload, Pairing, Arc<Phone>)>,
    /// The endpoint used for the link, and whether it is relay-only.
    phone: Option<(Arc<Phone>, bool)>,
    link: Option<Link>,
}

/// Keeps the secret key private to this module.
mod iroh_key {
    #[derive(Clone)]
    pub struct Secret(pub(super) notdot_link::transport::SecretKey);
}

/// Close an endpoint once nothing else uses it.
async fn retire(phone: Arc<Phone>) {
    if let Ok(phone) = Arc::try_unwrap(phone) {
        phone.close().await;
    }
}

/// Network configuration for a desktop's QR. Debug builds also accept a
/// plain-HTTP local development relay; release builds require https.
fn net_for(qr: &QrPayload, direct: bool) -> Result<NetConfig, MobileError> {
    let relays = match qr.relay.as_ref() {
        Some(url) => Some(
            notdot_link::transport::relay_map(url, cfg!(debug_assertions)).map_err(link_error)?,
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

/// The lock guards bookkeeping only: binding, pairing and connecting run
/// without it, so `route()` and `disconnect()` never wait on the network.
#[derive(uniffi::Object)]
pub struct PhoneLink {
    state: Arc<Mutex<State>>,
}

#[uniffi::export(async_runtime = "tokio")]
impl PhoneLink {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Arc::new(Mutex::new(State {
                secret: None,
                paired: None,
                pending: None,
                phone: None,
                link: None,
            })),
        })
    }

    /// Start pairing from a scanned (or typed) QR code. Returns the
    /// six-digit code to show; the desktop shows the same code. Nothing
    /// changes for an existing pairing unless the owner approves this one.
    pub async fn pair(&self, qr: String, name: String) -> Result<String, MobileError> {
        let qr = QrPayload::decode(qr.trim()).map_err(link_error)?;
        let net = net_for(&qr, true)?;
        let secret = self.state.lock().await.secret.clone();
        let phone = Arc::new(
            Phone::bind(secret.map(|s| s.0), &net)
                .await
                .map_err(link_error)?,
        );
        let pairing = match phone
            .pair(&qr, desktop_addr(&qr).map_err(link_error)?, &name, &[])
            .await
        {
            Ok(pairing) => pairing,
            Err(error) => {
                retire(phone).await;
                return Err(link_error(error));
            }
        };
        let sas = pairing.sas.to_string();
        let previous = {
            let mut state = self.state.lock().await;
            state
                .secret
                .get_or_insert_with(|| iroh_key::Secret(phone.secret()));
            state.pending.replace((qr, pairing, phone))
        };
        if let Some((_, _, old)) = previous {
            retire(old).await;
        }
        Ok(sas)
    }

    /// Wait for the owner to approve or deny on the desktop.
    pub async fn await_approval(&self) -> Result<PairResult, MobileError> {
        let (qr, pairing, phone) = self
            .state
            .lock()
            .await
            .pending
            .take()
            .ok_or_else(|| link_error("no pairing in progress"))?;
        let outcome = pairing.outcome().await;
        let result = match outcome {
            Ok(PairFrame::Approved { profile, .. }) => PairResult::Approved { profile },
            Ok(PairFrame::Denied) => PairResult::Denied,
            Ok(PairFrame::Rejected { reason }) => PairResult::Rejected { reason },
            Ok(PairFrame::AwaitingApproval { .. }) => PairResult::Rejected {
                reason: "unexpected reply".into(),
            },
            Err(error) => {
                retire(phone).await;
                return Err(link_error(error));
            }
        };
        if matches!(result, PairResult::Approved { .. }) {
            let (old_phone, old_link) = {
                let mut state = self.state.lock().await;
                state.paired = Some(qr);
                (state.phone.replace((phone, false)), state.link.take())
            };
            if let Some(link) = old_link {
                link.close();
            }
            if let Some((old, _)) = old_phone {
                retire(old).await;
            }
        } else {
            retire(phone).await;
        }
        Ok(result)
    }

    /// Connect to the paired desktop. `relay_only` drops direct paths, to
    /// prove the relay route (and to model a phone off the home network).
    pub async fn connect(&self, relay_only: bool) -> Result<LinkRoute, MobileError> {
        let (qr, current, secret) = {
            let state = self.state.lock().await;
            let qr = state
                .paired
                .clone()
                .ok_or_else(|| link_error("not paired"))?;
            let current = state
                .phone
                .as_ref()
                .filter(|(_, mode)| *mode == relay_only)
                .map(|(phone, _)| phone.clone());
            (qr, current, state.secret.clone())
        };
        let (phone, fresh) = match current {
            Some(phone) => (phone, false),
            None => (
                Arc::new(
                    Phone::bind(secret.map(|s| s.0), &net_for(&qr, !relay_only)?)
                        .await
                        .map_err(link_error)?,
                ),
                true,
            ),
        };
        let mut addr = desktop_addr(&qr).map_err(link_error)?;
        if relay_only {
            addr = notdot_link::transport::relay_only(&addr);
        }
        let link = match phone.connect(addr).await {
            Ok(link) => link,
            Err(error) => {
                if fresh {
                    retire(phone).await;
                }
                return Err(link_error(error));
            }
        };
        let route = route(&link);
        let (old_phone, old_link) = {
            let mut state = self.state.lock().await;
            let old_phone = if fresh {
                state.phone.replace((phone, relay_only))
            } else {
                None
            };
            (old_phone, state.link.replace(link))
        };
        if let Some(link) = old_link {
            link.close();
        }
        if let Some((old, _)) = old_phone {
            retire(old).await;
        }
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
            detail: "request id must be 32 hex digits".into(),
        });
    }
    u128::from_str_radix(text, 16)
        .map(RequestId)
        .map_err(|_| MobileError::Invalid {
            detail: "request id must be 32 hex digits".into(),
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
