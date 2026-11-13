//! The phone side of pairing and the encrypted link, for native shells.
//!
//! Identity: [`PhoneLink::new`] creates an in-memory identity (a restart
//! means pairing again). [`PhoneLink::restore`] takes a 32-byte identity
//! that native code keeps in this-device-only secure storage (Keychain on
//! iOS; a Keystore-wrapped file on Android), plus the [`PairedDesktop`]
//! record from [`PhoneLink::paired_record`], so a restarted app reconnects
//! without pairing again. This API never returns the identity key; native
//! code creates it ([`new_identity`]) and stores it before first use.
use crate::MobileError;
use notdot_link::{
    pairing::{PairedDesktop, QrPayload},
    protocol::{Op, PairFrame, RequestId, ServerFrame},
    transport::{Link, NetConfig, Pairing, Phone, Route, SecretKey, paired_addr},
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
    paired: Option<PairedDesktop>,
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

/// Network configuration for a desktop. Debug builds also accept a
/// plain-HTTP local development relay; release builds require https.
fn net_for(desktop: &PairedDesktop, direct: bool) -> Result<NetConfig, MobileError> {
    let relays = match desktop.relay.as_ref() {
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
    /// A phone with a new in-memory identity (lost when the app exits).
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Self::with(None, None)
    }

    /// A phone with a stored identity (32 bytes from [`new_identity`]) and,
    /// if it was paired, the record from [`PhoneLink::paired_record`]. A
    /// revoked phone restored this way is refused by the desktop on
    /// connect and must pair again.
    #[uniffi::constructor]
    pub fn restore(identity: Vec<u8>, paired: Option<String>) -> Result<Arc<Self>, MobileError> {
        let bytes: [u8; 32] = identity
            .as_slice()
            .try_into()
            .map_err(|_| MobileError::Invalid {
                detail: "identity must be 32 bytes".into(),
            })?;
        let paired = paired
            .map(|text| PairedDesktop::decode(text.trim()).map_err(link_error))
            .transpose()?;
        Ok(Self::with(
            Some(iroh_key::Secret(SecretKey::from_bytes(&bytes))),
            paired,
        ))
    }

    /// The record to store after approval (no secrets), or none if unpaired.
    pub async fn paired_record(&self) -> Option<String> {
        self.state
            .lock()
            .await
            .paired
            .as_ref()
            .map(PairedDesktop::encode)
    }

    /// Forget the pairing on this phone (the desktop must also revoke it).
    pub async fn unpair(&self) {
        let (link, phone) = {
            let mut state = self.state.lock().await;
            state.paired = None;
            (state.link.take(), state.phone.take())
        };
        if let Some(link) = link {
            link.close();
        }
        if let Some((phone, _)) = phone {
            retire(phone).await;
        }
    }

    /// Start pairing from a scanned (or typed) QR code. Returns the
    /// six-digit code to show; the desktop shows the same code. Nothing
    /// changes for an existing pairing unless the owner approves this one.
    pub async fn pair(&self, qr: String, name: String) -> Result<String, MobileError> {
        let qr = QrPayload::decode(qr.trim()).map_err(link_error)?;
        let net = net_for(&PairedDesktop::from_qr(&qr), true)?;
        let secret = self.state.lock().await.secret.clone();
        let phone = Arc::new(
            Phone::bind(secret.map(|s| s.0), &net)
                .await
                .map_err(link_error)?,
        );
        let pairing = match phone
            .pair(
                &qr,
                paired_addr(&PairedDesktop::from_qr(&qr)).map_err(link_error)?,
                &name,
                &[],
            )
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
                state.paired = Some(PairedDesktop::from_qr(&qr));
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
        let mut addr = paired_addr(&qr).map_err(link_error)?;
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
    fn with(secret: Option<iroh_key::Secret>, paired: Option<PairedDesktop>) -> Arc<Self> {
        Arc::new(Self {
            state: Arc::new(Mutex::new(State {
                secret,
                paired,
                pending: None,
                phone: None,
                link: None,
            })),
        })
    }

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

/// A new phone identity key (32 random bytes) for native secure storage.
/// Create it once, store it this-device-only, then pass it to
/// [`PhoneLink::restore`].
#[uniffi::export]
pub fn new_identity() -> Vec<u8> {
    SecretKey::generate().to_bytes().to_vec()
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
    use notdot_link::{
        pairing::{DeviceProfile, OFFER_TTL_MS, ProfileRef},
        service::system_clock,
        transport::Desktop,
    };
    use std::time::Duration;

    /// A local desktop on loopback: direct paths only, no relay, nothing
    /// leaves this machine. Profiles and data are synthetic.
    async fn desktop() -> Desktop {
        let net = NetConfig {
            relays: None,
            direct: true,
            insecure_test_relay: false,
            relay_roots: Vec::new(),
        };
        let desktop = Desktop::bind(None, &net, system_clock()).await.unwrap();
        desktop.set_profile_epoch("owner", 1);
        desktop.set_profile_epoch("member", 1);
        desktop
    }

    /// Pair `phone` into `profile`, approving on the desktop as the owner
    /// would after comparing codes. Returns the phone's device id.
    async fn pair(desktop: &Desktop, phone: &PhoneLink, profile: &str) -> notdot_link::DeviceId {
        let kind = if profile == "owner" {
            DeviceProfile::Owner
        } else {
            DeviceProfile::Member
        };
        let qr = desktop
            .create_offer(
                ProfileRef {
                    id: profile.into(),
                    epoch: 1,
                },
                kind,
                OFFER_TTL_MS,
            )
            .unwrap();
        let sas = phone
            .pair(qr.encode(), "Synthetic phone".into())
            .await
            .unwrap();
        let pending = desktop.pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].sas.to_string(),
            sas,
            "both screens show one code"
        );
        desktop.approve(&pending[0]).unwrap();
        assert!(matches!(
            phone.await_approval().await.unwrap(),
            PairResult::Approved { .. }
        ));
        pending[0].device
    }

    async fn within<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::time::timeout(Duration::from_secs(30), future)
            .await
            .expect("timed out")
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_restarted_phone_reconnects_with_its_stored_identity() {
        within(async {
            let desktop = desktop().await;
            let identity = new_identity();
            let phone = PhoneLink::restore(identity.clone(), None).unwrap();
            assert!(phone.paired_record().await.is_none());
            pair(&desktop, &phone, "owner").await;
            let record = phone.paired_record().await.expect("record after approval");
            phone.connect(false).await.unwrap();
            phone
                .add_note(new_request_id(), "before restart".into())
                .await
                .unwrap();
            // The app exits: everything in memory is gone.
            phone.disconnect().await;
            drop(phone);

            let restarted = PhoneLink::restore(identity, Some(record.clone())).unwrap();
            restarted.connect(false).await.unwrap();
            restarted
                .add_note(new_request_id(), "after restart".into())
                .await
                .unwrap();
            let notes = restarted.list_notes().await.unwrap();
            assert!(notes.contains(&"before restart".to_owned()), "{notes:?}");
            assert!(notes.contains(&"after restart".to_owned()), "{notes:?}");

            // The record alone grants nothing: another identity is refused.
            let stranger = PhoneLink::restore(new_identity(), Some(record)).unwrap();
            let refused = match stranger.connect(false).await {
                Err(_) => true,
                Ok(_) => stranger.list_notes().await.is_err(),
            };
            assert!(refused, "an unpaired identity must not reach the desktop");
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_revoked_phone_is_refused_after_restart_and_can_pair_again() {
        within(async {
            let desktop = desktop().await;
            let identity = new_identity();
            let phone = PhoneLink::restore(identity.clone(), None).unwrap();
            let device = pair(&desktop, &phone, "owner").await;
            let record = phone.paired_record().await.unwrap();
            drop(phone);

            desktop.revoke(device);
            let restarted = PhoneLink::restore(identity.clone(), Some(record)).unwrap();
            let refused = match restarted.connect(false).await {
                Err(_) => true,
                Ok(_) => restarted.list_notes().await.is_err(),
            };
            assert!(refused, "a revoked phone must be refused");

            // Pairing again (owner approves a new code) restores access.
            restarted.unpair().await;
            assert!(restarted.paired_record().await.is_none());
            pair(&desktop, &restarted, "owner").await;
            restarted.connect(false).await.unwrap();
            restarted.list_notes().await.unwrap();
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn phones_in_different_profiles_do_not_see_each_others_notes() {
        within(async {
            let desktop = desktop().await;
            let owner = PhoneLink::restore(new_identity(), None).unwrap();
            let member = PhoneLink::restore(new_identity(), None).unwrap();
            pair(&desktop, &owner, "owner").await;
            pair(&desktop, &member, "member").await;
            owner.connect(false).await.unwrap();
            member.connect(false).await.unwrap();
            owner
                .add_note(new_request_id(), "owner only".into())
                .await
                .unwrap();
            member
                .add_note(new_request_id(), "member only".into())
                .await
                .unwrap();
            assert_eq!(owner.list_notes().await.unwrap(), ["owner only"]);
            assert_eq!(member.list_notes().await.unwrap(), ["member only"]);
        })
        .await;
    }

    #[test]
    fn identities_must_be_32_bytes_and_records_well_formed() {
        assert!(PhoneLink::restore(vec![0; 31], None).is_err());
        assert!(PhoneLink::restore(new_identity(), Some("notdot-paired:1:!".into())).is_err());
        assert_ne!(new_identity(), new_identity());
    }

    #[test]
    fn request_ids_round_trip_and_bad_ones_are_refused() {
        let id = new_request_id();
        assert_eq!(format!("{:032x}", parse_id(&id).unwrap().0), id);
        assert!(parse_id("xyz").is_err());
        assert!(parse_id(&"g".repeat(32)).is_err());
    }
}
