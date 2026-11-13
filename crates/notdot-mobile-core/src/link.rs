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
use crate::stepup::{StepUpKey, StepUpSigner, check_step_up_key};
use notdot_link::{
    pairing::{PairedDesktop, QrPayload},
    protocol::{Op, OpError, PairFrame, RequestId, ServerFrame, StepUpProof},
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
    /// `step_up` is this phone's hardware step-up key, if it has one; it
    /// must be hardware-backed ([`check_step_up_key`]). Without one, the
    /// phone pairs but cannot perform sensitive actions.
    pub async fn pair(
        &self,
        qr: String,
        name: String,
        step_up: Option<StepUpKey>,
    ) -> Result<String, MobileError> {
        if let Some(key) = &step_up {
            check_step_up_key(key.clone())?;
        }
        let step_up_key = step_up.map(|key| key.public_key).unwrap_or_default();
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
                &step_up_key,
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
                return Err(self.failed(error).await);
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

    /// Clear this profile's memories on the desktop: a sensitive action.
    /// Needs a step-up key registered at pairing and an owner grant made at
    /// the desktop. `signer` asks the owner to authenticate on the phone
    /// once, for this one action. Returns how many memories were removed.
    pub async fn reset_memory(
        &self,
        request_id: String,
        signer: Arc<dyn StepUpSigner>,
    ) -> Result<u64, MobileError> {
        let id = parse_id(&request_id)?;
        let op = Op::ResetMemory {};
        let proof = self.step_up(&op, signer).await?;
        match self.call_with(id, op, Some(proof)).await? {
            ServerFrame::Done { result, .. } => Ok(result["removed"].as_u64().unwrap_or(0)),
            other => Err(refused(other)),
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
        self.call_with(id, op, None).await
    }

    async fn call_with(
        &self,
        id: RequestId,
        op: Op,
        step_up: Option<StepUpProof>,
    ) -> Result<ServerFrame, MobileError> {
        let link = self
            .state
            .lock()
            .await
            .link
            .clone()
            .ok_or_else(|| link_error("not connected"))?;
        match link.call(id, op, step_up).await {
            Ok(frame) => Ok(frame),
            Err(error) => Err(self.failed(error).await),
        }
    }

    /// Map a link failure for native code. A refusal (removed at the
    /// desktop, or a stale profile) ends this pairing: the core forgets it
    /// and says so, so native code can wipe its stored keys.
    async fn failed(&self, error: notdot_link::LinkError) -> MobileError {
        let notdot_link::LinkError::Refused(refusal) = error else {
            return link_error(error);
        };
        self.unpair().await;
        MobileError::Unpaired {
            detail: format!(
                "The computer no longer accepts this phone ({refusal}). Link it again to continue."
            ),
        }
    }

    /// Fetch a single-use challenge for `op` and have the owner sign it.
    async fn step_up(
        &self,
        op: &Op,
        signer: Arc<dyn StepUpSigner>,
    ) -> Result<StepUpProof, MobileError> {
        let digest = notdot_link::stepup::op_digest(op);
        let (desktop, phone) = {
            let state = self.state.lock().await;
            let desktop = state.paired.as_ref().map(|p| p.desktop);
            let phone = state.phone.as_ref().map(|(phone, _)| phone.id());
            desktop
                .zip(phone)
                .ok_or_else(|| link_error("not connected"))?
        };
        let nonce = match self
            .call(RequestId::random(), Op::StepUpChallenge { digest })
            .await?
        {
            ServerFrame::Done { result, .. } => notdot_link::stepup::challenge_nonce(&result)
                .ok_or_else(|| link_error("the desktop sent a malformed challenge"))?,
            other => return Err(refused(other)),
        };
        let message = notdot_link::stepup::message(desktop, phone, &nonce, &digest);
        // The owner may take a while to authenticate; keep the runtime free.
        let signature = tokio::task::spawn_blocking(move || signer.sign(message))
            .await
            .map_err(|_| link_error("step-up signing stopped"))??;
        Ok(StepUpProof { nonce, signature })
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

/// A plain-language error for a call the desktop did not commit.
fn refused(frame: ServerFrame) -> MobileError {
    link_error(match frame {
        ServerFrame::Failed { error, .. } => match error {
            OpError::Forbidden => {
                "This phone is not allowed to do that. The owner can grant it at the desktop."
                    .into()
            }
            OpError::NeedsStepUp | OpError::StepUpRejected => {
                "The desktop did not accept this phone's confirmation. Try again.".into()
            }
            OpError::Busy => "The desktop is busy. Try again.".into(),
            other => format!("The desktop refused: {other:?}"),
        },
        ServerFrame::Cancelled { .. } => "Cancelled; nothing changed.".into(),
        other => format!("Unexpected reply: {other:?}"),
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
        pair_with(desktop, phone, profile, None).await
    }

    async fn pair_with(
        desktop: &Desktop,
        phone: &PhoneLink,
        profile: &str,
        step_up: Option<StepUpKey>,
    ) -> notdot_link::DeviceId {
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
            .pair(qr.encode(), "Synthetic phone".into(), step_up)
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
            // Refused, reported as such (so native code wipes its keys),
            // and the pairing is forgotten.
            let refusal = match restarted.connect(false).await {
                Err(error) => error,
                Ok(_) => restarted.list_notes().await.unwrap_err(),
            };
            assert!(
                matches!(refusal, MobileError::Unpaired { .. }),
                "{refusal:?}"
            );
            assert!(restarted.paired_record().await.is_none());

            // Pairing again (owner approves a new code) restores access.
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

    /// A stand-in for the native hardware signer, for loopback tests only:
    /// a software P-256 key that claims a hardware protection level. Real
    /// Secure Enclave / StrongBox behaviour needs a physical phone.
    struct MockSigner {
        key: p256::ecdsa::SigningKey,
        fail: bool,
        signed: std::sync::Mutex<Vec<Vec<u8>>>,
    }
    impl MockSigner {
        fn new(seed: u8) -> Arc<Self> {
            Arc::new(Self {
                key: p256::ecdsa::SigningKey::from_slice(&[seed; 32]).unwrap(),
                fail: false,
                signed: Default::default(),
            })
        }
        fn public(&self) -> StepUpKey {
            StepUpKey {
                public_key: self
                    .key
                    .verifying_key()
                    .to_encoded_point(false)
                    .as_bytes()
                    .to_vec(),
                protection: crate::stepup::KeyProtection::TrustedEnvironment,
            }
        }
    }
    impl StepUpSigner for MockSigner {
        fn sign(&self, message: Vec<u8>) -> Result<Vec<u8>, MobileError> {
            use p256::ecdsa::signature::Signer;
            self.signed.lock().unwrap().push(message.clone());
            if self.fail {
                return Err(MobileError::Invalid {
                    detail: "The owner cancelled.".into(),
                });
            }
            let signature: p256::ecdsa::Signature = self.key.sign(&message);
            Ok(signature.to_der().as_bytes().to_vec())
        }
    }

    /// The phone's step-up path end to end over loopback QUIC: refused
    /// without the desktop's grant, refused when signed by another key,
    /// refused when the owner cancels, accepted once with the registered
    /// key, and each attempt asks the owner exactly once.
    #[tokio::test(flavor = "multi_thread")]
    async fn sensitive_actions_need_the_grant_and_the_registered_hardware_key() {
        within(async {
            let desktop = desktop().await;
            let signer = MockSigner::new(7);
            let phone = PhoneLink::new();
            let device = pair_with(&desktop, &phone, "owner", Some(signer.public())).await;
            phone.connect(false).await.unwrap();

            // No owner grant yet: refused before the owner is asked.
            assert!(
                phone
                    .reset_memory(new_request_id(), signer.clone())
                    .await
                    .is_err()
            );
            assert!(signer.signed.lock().unwrap().is_empty());

            desktop.grant_sensitive(device, u64::MAX).unwrap();
            // Another key (say, a different phone's) is refused.
            let other = MockSigner::new(9);
            assert!(
                phone
                    .reset_memory(new_request_id(), other.clone())
                    .await
                    .is_err()
            );
            assert_eq!(other.signed.lock().unwrap().len(), 1);
            // The owner cancels: nothing is sent.
            let cancelled = Arc::new(MockSigner {
                key: p256::ecdsa::SigningKey::from_slice(&[7; 32]).unwrap(),
                fail: true,
                signed: Default::default(),
            });
            assert!(
                phone
                    .reset_memory(new_request_id(), cancelled)
                    .await
                    .is_err()
            );
            // The registered key: accepted, asking once.
            assert_eq!(
                phone
                    .reset_memory(new_request_id(), signer.clone())
                    .await
                    .unwrap(),
                0
            );
            let signed = signer.signed.lock().unwrap().clone();
            assert_eq!(signed.len(), 1);
            // Bound to this desktop and this phone (see notdot_link::stepup).
            assert_eq!(&signed[0][17..49], &desktop.id().0);
            assert_eq!(&signed[0][49..81], &device.0);
        })
        .await;
    }

    /// A software key is never registered, and a phone paired without a
    /// hardware key cannot be granted sensitive actions.
    #[tokio::test(flavor = "multi_thread")]
    async fn no_silent_downgrade_to_a_software_step_up_key() {
        within(async {
            let desktop = desktop().await;
            let signer = MockSigner::new(7);
            let phone = PhoneLink::new();
            let mut software = signer.public();
            software.protection = crate::stepup::KeyProtection::Software;
            let qr = desktop
                .create_offer(
                    ProfileRef {
                        id: "owner".into(),
                        epoch: 1,
                    },
                    DeviceProfile::Owner,
                    OFFER_TTL_MS,
                )
                .unwrap();
            assert!(
                phone
                    .pair(qr.encode(), "Synthetic phone".into(), Some(software))
                    .await
                    .is_err()
            );
            assert!(desktop.pending().is_empty(), "nothing reached the desktop");

            let device = pair(&desktop, &phone, "owner").await;
            // The desktop will not even grant sensitive access to it.
            assert!(desktop.grant_sensitive(device, u64::MAX).is_err());
            phone.connect(false).await.unwrap();
            assert!(
                phone
                    .reset_memory(new_request_id(), signer.clone())
                    .await
                    .is_err()
            );
            assert!(signer.signed.lock().unwrap().is_empty());
        })
        .await;
    }

    /// Removed at the desktop while connected: the next call reports the
    /// phone unpaired and the pairing is forgotten, so no retry loop.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_phone_removed_while_connected_learns_it_is_unpaired() {
        within(async {
            let desktop = desktop().await;
            let phone = PhoneLink::restore(new_identity(), None).unwrap();
            let device = pair(&desktop, &phone, "owner").await;
            phone.connect(false).await.unwrap();
            phone.list_notes().await.unwrap();
            desktop.revoke(device);
            let error = phone.list_notes().await.unwrap_err();
            assert!(matches!(error, MobileError::Unpaired { .. }), "{error:?}");
            assert!(phone.paired_record().await.is_none());
            assert!(phone.connect(false).await.is_err(), "not paired any more");
        })
        .await;
    }
}
