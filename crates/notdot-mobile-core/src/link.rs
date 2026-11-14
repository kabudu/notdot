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
use crate::stepup::{
    StepUpAttester, StepUpProtection, StepUpSigner, StepUpStatus, attest_request, sign_request,
};
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
    /// Sensitive actions need a separate, attested step-up key
    /// ([`Self::set_up_step_up`]).
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

    /// This phone's step-up state on the computer.
    pub async fn step_up_status(&self) -> Result<StepUpStatus, MobileError> {
        match self.call(RequestId::random(), Op::StepUpStatus {}).await? {
            ServerFrame::Done { result, .. } => Ok(StepUpStatus {
                enrolled: result["enrolled"].as_bool().unwrap_or(false),
                protection: StepUpProtection::from_wire(&result["protection"]),
                granted: result["granted"].as_bool().unwrap_or(false),
                test_only: result["test_only"].as_bool().unwrap_or(false),
                development: result["development"].as_bool().unwrap_or(false),
                key_sha256: result["key_sha256"]
                    .as_str()
                    .and_then(notdot_link::b64_decode)
                    .filter(|d| d.len() == 32),
            }),
            other => Err(refused(other)),
        }
    }

    /// Set up (or replace) this phone's step-up key. `attester` creates the
    /// key and its platform evidence; the computer verifies the evidence
    /// and returns the protection it confirmed. A new key ends any earlier
    /// grant: the owner allows sensitive actions again at the computer.
    ///
    /// Native code keeps the previous key until this succeeds. If it fails
    /// with [`MobileError::Offline`], the computer may or may not have the
    /// new key: keep both, and settle it later from
    /// [`StepUpStatus::key_sha256`] (see [`crate::stepup::step_up_key_sha256`]).
    pub async fn set_up_step_up(
        &self,
        attester: Arc<dyn StepUpAttester>,
    ) -> Result<StepUpProtection, MobileError> {
        let (desktop, phone) = self.ids().await?;
        let nonce = match self
            .call(RequestId::random(), Op::StepUpEnrollChallenge {})
            .await?
        {
            ServerFrame::Done { result, .. } => notdot_link::stepup::challenge_nonce(&result)
                .ok_or_else(|| link_error("the computer sent a malformed challenge"))?,
            other => return Err(refused(other)),
        };
        let request = attest_request(desktop, phone, nonce);
        // Key generation and App Attest can take a while; keep the runtime free.
        let evidence = tokio::task::spawn_blocking(move || attester.attest(request))
            .await
            .map_err(|_| link_error("setting up the key stopped"))??
            .into_wire()?;
        match self
            .call(RequestId::random(), Op::StepUpEnroll { nonce, evidence })
            .await?
        {
            ServerFrame::Done { result, .. } => StepUpProtection::from_wire(&result["protection"])
                .ok_or_else(|| link_error("the computer sent a malformed reply")),
            other => Err(refused(other)),
        }
    }

    /// Clear this profile's memories on the computer: a sensitive action.
    /// Needs an attested step-up key and the owner's grant at the computer.
    /// `signer` asks the owner to authenticate on the phone once, for this
    /// one action. Returns how many memories were removed.
    ///
    /// Call again with the same `request_id` after a dropped connection: if
    /// the computer already did it, the recorded result comes back without
    /// a second prompt or a second effect.
    pub async fn reset_memory(
        &self,
        request_id: String,
        signer: Arc<dyn StepUpSigner>,
    ) -> Result<u64, MobileError> {
        let id = parse_id(&request_id)?;
        let op = Op::ResetMemory {};
        let removed = |frame: ServerFrame| match frame {
            ServerFrame::Done { result, .. } => Ok(result["removed"].as_u64().unwrap_or(0)),
            other => Err(refused(other)),
        };
        // Already done under this id? Then no prompt is needed.
        match self.call(id, op.clone()).await? {
            ServerFrame::Failed {
                error: OpError::NeedsStepUp,
                ..
            } => {}
            other => return removed(other),
        }
        let proof = self.step_up(&op, signer).await?;
        removed(self.call_with(id, op, Some(proof)).await?)
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
        let link = self.state.lock().await.link.clone().ok_or_else(offline)?;
        match link.call(id, op, step_up).await {
            Ok(frame) => Ok(frame),
            Err(error) => Err(self.failed(error).await),
        }
    }

    /// Map a link failure for native code. A refusal (removed at the
    /// desktop, or a stale profile) ends this pairing: the core forgets it
    /// and says so, so native code can wipe its stored keys.
    async fn failed(&self, error: notdot_link::LinkError) -> MobileError {
        let refusal = match error {
            notdot_link::LinkError::Refused(refusal) => refusal,
            notdot_link::LinkError::Connection(_) | notdot_link::LinkError::Io(_) => {
                return MobileError::Offline {
                    detail: "Lost the connection to the computer. Nothing was lost; try again."
                        .into(),
                };
            }
            other => return link_error(other),
        };
        self.unpair().await;
        MobileError::Unpaired {
            detail: format!(
                "The computer no longer accepts this phone ({refusal}). Link it again to continue."
            ),
        }
    }

    async fn ids(&self) -> Result<(notdot_link::DeviceId, notdot_link::DeviceId), MobileError> {
        let state = self.state.lock().await;
        let desktop = state.paired.as_ref().map(|p| p.desktop);
        let phone = state.phone.as_ref().map(|(phone, _)| phone.id());
        desktop.zip(phone).ok_or_else(offline)
    }

    /// Fetch a single-use challenge for `op` and have the owner sign it.
    async fn step_up(
        &self,
        op: &Op,
        signer: Arc<dyn StepUpSigner>,
    ) -> Result<StepUpProof, MobileError> {
        let digest = notdot_link::stepup::op_digest(op);
        let (desktop, phone) = self.ids().await?;
        let nonce = match self
            .call(RequestId::random(), Op::StepUpChallenge { digest })
            .await?
        {
            ServerFrame::Done { result, .. } => notdot_link::stepup::challenge_nonce(&result)
                .ok_or_else(|| link_error("the computer sent a malformed challenge"))?,
            other => return Err(refused(other)),
        };
        let request = sign_request(notdot_link::stepup::message(
            desktop, phone, &nonce, &digest,
        ));
        // The owner may take a while to authenticate; keep the runtime free.
        let signed = tokio::task::spawn_blocking(move || signer.sign(request))
            .await
            .map_err(|_| link_error("step-up signing stopped"))??;
        Ok(StepUpProof {
            nonce,
            signature: signed.signature,
            assertion: signed.assertion,
        })
    }
}

fn offline() -> MobileError {
    MobileError::Offline {
        detail: "Not connected to the computer.".into(),
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

/// A plain-language error for a call the computer did not commit.
fn refused(frame: ServerFrame) -> MobileError {
    let denied = |detail: &str| MobileError::Denied {
        detail: detail.into(),
    };
    match frame {
        ServerFrame::Failed { error, .. } => match error {
            OpError::Forbidden => denied(
                "This phone isn't allowed to do that right now. Allow sensitive actions for it on your computer, then try again.",
            ),
            OpError::NeedsStepUp | OpError::StepUpRejected => {
                denied("Your computer didn't accept this phone's confirmation. Nothing changed.")
            }
            OpError::StepUpExpired => MobileError::Expired {
                detail: "The confirmation took too long. Nothing changed; start again.".into(),
            },
            OpError::StepUpNotEnrolled => MobileError::Unavailable {
                detail: "Set up sensitive actions on this phone first.".into(),
            },
            OpError::AttestationRejected { reason } => MobileError::Denied {
                detail: format!("Your computer couldn't verify this phone: {reason}."),
            },
            OpError::Busy => link_error("Your computer is busy. Try again in a moment."),
            OpError::Interrupted => link_error(
                "Your computer restarted while doing this, so it may or may not have happened. Check before trying again.",
            ),
            OpError::NotAvailable { reason } => MobileError::Unavailable {
                detail: format!("Your computer can't do that: {reason}."),
            },
            other => link_error(format!("Your computer refused: {other:?}")),
        },
        ServerFrame::Cancelled { .. } => MobileError::Cancelled {
            detail: "Cancelled; nothing changed.".into(),
        },
        other => link_error(format!("Unexpected reply: {other:?}")),
    }
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

            desktop.revoke(device).unwrap();
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
    use crate::stepup::{
        AttestRequest, SignRequest, StepUpEvidence, StepUpSignature, app_attest_client_data_hash,
    };
    use notdot_link::attest::testing::{AppAttestDevice, KeySpec, TestRoots, point, random_key};
    use p256::ecdsa::{Signature, SigningKey, signature::Signer as _};
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    /// Test-only native adapters: synthetic keys and evidence from a test
    /// CA. The desktop marks what they enrol `test_only`; they never stand
    /// in for real hardware.
    struct TestKeys {
        roots: TestRoots,
        apple: bool,
        key: std::sync::Mutex<Option<SigningKey>>,
        device: std::sync::Mutex<AppAttestDevice>,
        signed: AtomicUsize,
        cancel: std::sync::atomic::AtomicBool,
        /// Called inside `sign`, as if the owner took a while.
        during_sign: std::sync::Mutex<Option<Box<dyn Fn() + Send>>>,
    }
    impl TestKeys {
        fn new(roots: &TestRoots, apple: bool) -> Arc<Self> {
            Arc::new(Self {
                device: std::sync::Mutex::new(roots.app_attest_device()),
                roots: roots.clone(),
                apple,
                key: std::sync::Mutex::new(None),
                signed: AtomicUsize::new(0),
                cancel: false.into(),
                during_sign: std::sync::Mutex::new(None),
            })
        }
    }
    impl StepUpAttester for TestKeys {
        fn attest(&self, request: AttestRequest) -> Result<StepUpEvidence, MobileError> {
            let key = random_key();
            let evidence = if self.apple {
                let hash: [u8; 32] = app_attest_client_data_hash(request, point(&key))?
                    .try_into()
                    .unwrap();
                let device = self.device.lock().unwrap();
                StepUpEvidence::AppleAppAttest {
                    step_up_key: point(&key),
                    key_id: device.key_id.to_vec(),
                    attestation: device.attest(&hash),
                }
            } else {
                let challenge: [u8; 32] = request.android_challenge.try_into().unwrap();
                StepUpEvidence::AndroidKeystore {
                    chain: self
                        .roots
                        .android_chain(&key, &KeySpec::good(challenge, &self.roots.android_app)),
                }
            };
            *self.key.lock().unwrap() = Some(key);
            Ok(evidence)
        }
    }
    impl StepUpSigner for TestKeys {
        fn sign(&self, request: SignRequest) -> Result<StepUpSignature, MobileError> {
            if let Some(hook) = self.during_sign.lock().unwrap().as_ref() {
                hook();
            }
            if self.cancel.load(Ordering::SeqCst) {
                return Err(MobileError::Cancelled {
                    detail: "cancelled".into(),
                });
            }
            self.signed.fetch_add(1, Ordering::SeqCst);
            let key = self
                .key
                .lock()
                .unwrap()
                .clone()
                .ok_or(MobileError::Unavailable {
                    detail: "no key".into(),
                })?;
            let signature: Signature = key.sign(&request.message);
            let assertion = self.apple.then(|| {
                let data = notdot_link::attest::assertion_client_data(&request.message);
                assert_eq!(
                    request.app_attest_client_data_hash,
                    <sha2::Sha256 as sha2::Digest>::digest(&data).to_vec()
                );
                self.device.lock().unwrap().assert(&data)
            });
            Ok(StepUpSignature {
                signature: signature.to_der().as_bytes().to_vec(),
                assertion,
            })
        }
    }

    /// A desktop on a controllable clock that trusts `roots`.
    async fn attesting_desktop(roots: &TestRoots, clock: Arc<AtomicU64>) -> Desktop {
        let net = NetConfig {
            relays: None,
            direct: true,
            insecure_test_relay: false,
            relay_roots: Vec::new(),
        };
        let desktop = Desktop::bind(None, &net, Arc::new(move || clock.load(Ordering::SeqCst)))
            .await
            .unwrap();
        desktop.set_profile_epoch("owner", 1);
        desktop.set_profile_epoch("member", 1);
        desktop.set_attestation_policy(roots.policy(now()));
        desktop
    }

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    async fn connected(
        desktop: &Desktop,
        profile: &str,
    ) -> (Arc<PhoneLink>, notdot_link::DeviceId, Vec<u8>) {
        let identity = new_identity();
        let phone = PhoneLink::restore(identity.clone(), None).unwrap();
        let device = pair(desktop, &phone, profile).await;
        phone.connect(false).await.unwrap();
        (phone, device, identity)
    }

    async fn flow(apple: bool) {
        let clock = Arc::new(AtomicU64::new(now()));
        let roots = TestRoots::new(now());
        let desktop = attesting_desktop(&roots, clock.clone()).await;
        let (phone, device, _) = connected(&desktop, "owner").await;
        let keys = TestKeys::new(&roots, apple);
        desktop.service().remember("owner", "likes tea");

        // Before set-up: nothing to confirm with.
        let status = phone.step_up_status().await.unwrap();
        assert!(!status.enrolled && !status.granted);
        let error = phone
            .reset_memory(new_request_id(), keys.clone())
            .await
            .unwrap_err();
        assert!(
            matches!(error, MobileError::Unavailable { .. }),
            "{error:?}"
        );
        assert!(
            desktop.grant_sensitive(device, now() + 600_000).is_err(),
            "no key, no grant"
        );

        let protection = phone.set_up_step_up(keys.clone()).await.unwrap();
        let expected = if apple {
            StepUpProtection::AppleAppAttested
        } else {
            StepUpProtection::AndroidTrustedEnvironment
        };
        assert_eq!(protection, expected);
        let status = phone.step_up_status().await.unwrap();
        assert_eq!(status.protection, Some(expected));
        assert!(status.enrolled && status.test_only && !status.granted);
        assert!(!status.development);
        let held = point(keys.key.lock().unwrap().as_ref().unwrap());
        assert_eq!(
            status.key_sha256,
            Some(crate::stepup::step_up_key_sha256(held))
        );

        // Set up but not allowed at the computer: denied, no prompt.
        let error = phone
            .reset_memory(new_request_id(), keys.clone())
            .await
            .unwrap_err();
        assert!(matches!(error, MobileError::Denied { .. }), "{error:?}");
        assert_eq!(keys.signed.load(Ordering::SeqCst), 0);

        desktop.grant_sensitive(device, now() + 600_000).unwrap();
        assert!(phone.step_up_status().await.unwrap().granted);

        // The owner cancels the prompt: nothing changes.
        keys.cancel.store(true, Ordering::SeqCst);
        let error = phone
            .reset_memory(new_request_id(), keys.clone())
            .await
            .unwrap_err();
        assert!(matches!(error, MobileError::Cancelled { .. }), "{error:?}");
        assert_eq!(desktop.service().profile("owner").memories.len(), 1);
        keys.cancel.store(false, Ordering::SeqCst);

        // Confirmed once: done once.
        let id = new_request_id();
        assert_eq!(
            phone.reset_memory(id.clone(), keys.clone()).await.unwrap(),
            1
        );
        assert_eq!(keys.signed.load(Ordering::SeqCst), 1);
        assert!(desktop.service().profile("owner").memories.is_empty());
        // A retry with the same id (say after a dropped reply) returns the
        // recorded result: no second prompt, no second effect.
        desktop.service().remember("owner", "likes coffee");
        assert_eq!(phone.reset_memory(id, keys.clone()).await.unwrap(), 1);
        assert_eq!(keys.signed.load(Ordering::SeqCst), 1);
        assert_eq!(desktop.service().profile("owner").memories.len(), 1);

        // Too slow to confirm: expired, nothing changed.
        {
            let clock = clock.clone();
            *keys.during_sign.lock().unwrap() = Some(Box::new(move || {
                clock.fetch_add(notdot_link::stepup::CHALLENGE_TTL_MS, Ordering::SeqCst);
            }));
        }
        let error = phone
            .reset_memory(new_request_id(), keys.clone())
            .await
            .unwrap_err();
        assert!(matches!(error, MobileError::Expired { .. }), "{error:?}");
        assert_eq!(desktop.service().profile("owner").memories.len(), 1);
        *keys.during_sign.lock().unwrap() = None;

        // Setting up a new key ends the grant.
        phone.set_up_step_up(keys.clone()).await.unwrap();
        assert!(!phone.step_up_status().await.unwrap().granted);
        let error = phone
            .reset_memory(new_request_id(), keys.clone())
            .await
            .unwrap_err();
        assert!(matches!(error, MobileError::Denied { .. }), "{error:?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn android_attested_set_up_grant_and_one_confirmed_action() {
        within(flow(false)).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn iphone_app_attested_set_up_grant_and_one_confirmed_action() {
        within(flow(true)).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn evidence_from_untrusted_roots_is_refused_by_a_real_policy() {
        within(async {
            let roots = TestRoots::new(now());
            let desktop = attesting_desktop(&roots, Arc::new(AtomicU64::new(now()))).await;
            // The genuine policy: Google and Apple roots, apps configured.
            let mut real = notdot_link::attest::AttestationPolicy::default();
            real.android_apps = vec![roots.android_app.clone()];
            real.apple = roots.policy(now()).apple;
            real.set_status_list(
                notdot_link::attest::android::StatusList::parse(br#"{"entries":{}}"#, now())
                    .unwrap(),
            );
            desktop.set_attestation_policy(real);
            let (phone, device, _) = connected(&desktop, "owner").await;
            for apple in [false, true] {
                let error = phone
                    .set_up_step_up(TestKeys::new(&roots, apple))
                    .await
                    .unwrap_err();
                assert!(
                    matches!(&error, MobileError::Denied { detail } if detail.contains("couldn't verify")),
                    "{error:?}"
                );
            }
            assert!(!phone.step_up_status().await.unwrap().enrolled);
            assert!(desktop.grant_sensitive(device, now() + 600_000).is_err());
            // An unconfigured desktop accepts nothing.
            desktop.set_attestation_policy(Default::default());
            let error = phone
                .set_up_step_up(TestKeys::new(&roots, false))
                .await
                .unwrap_err();
            assert!(matches!(error, MobileError::Denied { .. }), "{error:?}");
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn members_cannot_set_up_or_use_the_owners_access() {
        within(async {
            let roots = TestRoots::new(now());
            let desktop = attesting_desktop(&roots, Arc::new(AtomicU64::new(now()))).await;
            let (owner, owner_id, _) = connected(&desktop, "owner").await;
            let (member, member_id, _) = connected(&desktop, "member").await;
            let keys = TestKeys::new(&roots, false);
            owner.set_up_step_up(keys.clone()).await.unwrap();
            desktop.grant_sensitive(owner_id, now() + 600_000).unwrap();
            desktop.service().remember("member", "member memory");
            let error = member
                .set_up_step_up(TestKeys::new(&roots, false))
                .await
                .unwrap_err();
            assert!(matches!(error, MobileError::Denied { .. }), "{error:?}");
            assert!(desktop.grant_sensitive(member_id, now() + 600_000).is_err());
            // Even holding the owner's key, a member is refused.
            let error = member
                .reset_memory(new_request_id(), keys.clone())
                .await
                .unwrap_err();
            assert!(matches!(error, MobileError::Denied { .. }), "{error:?}");
            assert_eq!(desktop.service().profile("member").memories.len(), 1);
            // The owner's action touches only the owner's profile.
            desktop.service().remember("owner", "owner memory");
            assert_eq!(owner.reset_memory(new_request_id(), keys).await.unwrap(), 1);
            assert_eq!(desktop.service().profile("member").memories.len(), 1);
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_restarted_phone_keeps_its_key_and_removal_forgets_it() {
        within(async {
            let roots = TestRoots::new(now());
            let desktop = attesting_desktop(&roots, Arc::new(AtomicU64::new(now()))).await;
            let (phone, device, identity) = connected(&desktop, "owner").await;
            let keys = TestKeys::new(&roots, false);
            phone.set_up_step_up(keys.clone()).await.unwrap();
            desktop.grant_sensitive(device, now() + 600_000).unwrap();
            let record = phone.paired_record().await.unwrap();
            phone.disconnect().await;
            drop(phone);
            // Restart: the stored identity reconnects; the key still works.
            let restarted = PhoneLink::restore(identity.clone(), Some(record)).unwrap();
            restarted.connect(false).await.unwrap();
            desktop.service().remember("owner", "x");
            assert_eq!(
                restarted
                    .reset_memory(new_request_id(), keys.clone())
                    .await
                    .unwrap(),
                1
            );
            // Removal forgets the key; pairing again starts from scratch.
            desktop.revoke(device).unwrap();
            let error = restarted.step_up_status().await.unwrap_err();
            assert!(matches!(error, MobileError::Unpaired { .. }), "{error:?}");
            pair(&desktop, &restarted, "owner").await;
            restarted.connect(false).await.unwrap();
            assert!(!restarted.step_up_status().await.unwrap().enrolled);
            assert!(desktop.grant_sensitive(device, now() + 600_000).is_err());
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn offline_is_reported_as_offline() {
        within(async {
            let roots = TestRoots::new(now());
            let desktop = attesting_desktop(&roots, Arc::new(AtomicU64::new(now()))).await;
            let (phone, _, _) = connected(&desktop, "owner").await;
            phone.disconnect().await;
            let keys = TestKeys::new(&roots, false);
            for error in [
                phone.step_up_status().await.unwrap_err(),
                phone.set_up_step_up(keys.clone()).await.unwrap_err(),
                phone
                    .reset_memory(new_request_id(), keys)
                    .await
                    .unwrap_err(),
            ] {
                assert!(matches!(error, MobileError::Offline { .. }), "{error:?}");
            }
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
            desktop.revoke(device).unwrap();
            let error = phone.list_notes().await.unwrap_err();
            assert!(matches!(error, MobileError::Unpaired { .. }), "{error:?}");
            assert!(phone.paired_record().await.is_none());
            assert!(phone.connect(false).await.is_err(), "not paired any more");
        })
        .await;
    }

    /// A desktop that saves its state in `dir`, with a fixed identity.
    async fn saved_desktop(
        roots: &TestRoots,
        dir: &std::path::Path,
        secret: &notdot_link::transport::SecretKey,
        backend: Arc<dyn notdot_link::service::Backend>,
    ) -> Desktop {
        let net = NetConfig {
            relays: None,
            direct: true,
            insecure_test_relay: false,
            relay_roots: Vec::new(),
        };
        let desktop = Desktop::bind_with(
            Some(secret.clone()),
            &net,
            system_clock(),
            notdot_link::transport::DesktopOptions {
                backend: Some(backend),
                state_dir: Some(dir.to_path_buf()),
                epochs: Some(Arc::new(|p: &str| {
                    matches!(p, "owner" | "member").then_some(1)
                })),
            },
        )
        .await
        .unwrap();
        desktop.set_attestation_policy(roots.policy(now()));
        desktop
    }

    /// A backend whose memory reset never finishes, to stop the desktop
    /// part-way through a destructive call.
    struct Stuck(Arc<notdot_link::service::SyntheticBackend>);
    impl notdot_link::service::Backend for Stuck {
        fn add_note(
            &self,
            p: String,
            t: String,
        ) -> notdot_link::service::BoxFuture<Result<u64, OpError>> {
            self.0.add_note(p, t)
        }
        fn list_notes(
            &self,
            p: String,
        ) -> notdot_link::service::BoxFuture<Result<Vec<(u64, String)>, OpError>> {
            self.0.list_notes(p)
        }
        fn read_settings(
            &self,
            p: String,
        ) -> notdot_link::service::BoxFuture<
            Result<(u64, std::collections::BTreeMap<String, String>), OpError>,
        > {
            self.0.read_settings(p)
        }
        fn stage_setting(
            &self,
            p: String,
            k: String,
            v: String,
            r: u64,
        ) -> notdot_link::service::BoxFuture<Result<u64, OpError>> {
            self.0.stage_setting(p, k, v, r)
        }
        fn run_task(
            &self,
            p: String,
            l: String,
            n: u32,
            c: tokio::sync::watch::Receiver<bool>,
        ) -> notdot_link::service::BoxFuture<Result<Option<serde_json::Value>, OpError>> {
            self.0.run_task(p, l, n, c)
        }
        fn reset_memory(&self, _: String) -> notdot_link::service::BoxFuture<Result<u64, OpError>> {
            Box::pin(std::future::pending())
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn keys_grants_and_outcomes_survive_a_desktop_restart_and_nothing_runs_twice() {
        within(async {
            let roots = TestRoots::new(now());
            let dir = std::env::temp_dir().join(format!("notdot-restart-{}", new_request_id()));
            let secret = notdot_link::transport::SecretKey::generate();
            let store = Arc::new(notdot_link::service::SyntheticBackend::default());
            let desktop = saved_desktop(&roots, &dir, &secret, Arc::new(store.clone())).await;
            let (phone, device, identity) = connected(&desktop, "owner").await;
            let (member, member_id, _) = connected(&desktop, "member").await;
            let keys = TestKeys::new(&roots, false);
            phone.set_up_step_up(keys.clone()).await.unwrap();
            desktop.grant_sensitive(device, now() + 600_000).unwrap();
            store.remember("owner", "one");
            let erase = new_request_id();
            assert_eq!(
                phone
                    .reset_memory(erase.clone(), keys.clone())
                    .await
                    .unwrap(),
                1
            );
            let note = new_request_id();
            let first = member
                .add_note(note.clone(), "member note".into())
                .await
                .unwrap();
            let record = phone.paired_record().await.unwrap();
            phone.disconnect().await;
            member.disconnect().await;
            drop(phone);
            desktop.close().await;

            // Restart: same identity, same saved state, same data.
            let desktop = saved_desktop(&roots, &dir, &secret, Arc::new(store.clone())).await;
            assert_eq!(desktop.devices().len(), 2);
            let phone = PhoneLink::restore(identity, Some(record)).unwrap();
            phone.connect(false).await.unwrap();
            let status = phone.step_up_status().await.unwrap();
            assert!(status.enrolled && status.granted, "{status:?}");
            // The recorded erase comes back without a prompt or a second effect.
            store.remember("owner", "two");
            assert_eq!(phone.reset_memory(erase, keys.clone()).await.unwrap(), 1);
            assert_eq!(store.profile("owner").memories.len(), 1);
            assert_eq!(keys.signed.load(Ordering::SeqCst), 1);
            // So does the member's note, and the member still can't erase.
            member.connect(false).await.unwrap();
            assert_eq!(
                member.add_note(note, "member note".into()).await.unwrap(),
                first
            );
            assert_eq!(store.profile("member").notes.len(), 1);
            let error = member
                .reset_memory(new_request_id(), keys.clone())
                .await
                .unwrap_err();
            assert!(matches!(error, MobileError::Denied { .. }), "{error:?}");

            // Removal survives a restart too.
            desktop.revoke(member_id).unwrap();
            phone.disconnect().await;
            desktop.close().await;
            let desktop = saved_desktop(&roots, &dir, &secret, Arc::new(store.clone())).await;
            assert_eq!(desktop.devices().len(), 1);
            let error = match member.connect(false).await {
                Err(error) => error,
                Ok(_) => member.list_notes().await.unwrap_err(),
            };
            assert!(matches!(error, MobileError::Unpaired { .. }), "{error:?}");
            desktop.close().await;
            std::fs::remove_dir_all(&dir).unwrap();
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_erase_interrupted_by_a_restart_is_reported_never_rerun() {
        within(async {
            eprintln!("interrupted erase: prepare");
            let roots = TestRoots::new(now());
            let dir = std::env::temp_dir().join(format!("notdot-interrupt-{}", new_request_id()));
            let secret = notdot_link::transport::SecretKey::generate();
            let store = Arc::new(notdot_link::service::SyntheticBackend::default());
            let desktop =
                saved_desktop(&roots, &dir, &secret, Arc::new(Stuck(store.clone()))).await;
            eprintln!("interrupted erase: pair and connect");
            let (phone, device, identity) = connected(&desktop, "owner").await;
            let keys = TestKeys::new(&roots, false);
            eprintln!("interrupted erase: attest");
            phone.set_up_step_up(keys.clone()).await.unwrap();
            desktop.grant_sensitive(device, now() + 600_000).unwrap();
            store.remember("owner", "keep");
            let erase = new_request_id();
            let stuck = {
                let phone = phone.clone();
                let keys = keys.clone();
                let erase = erase.clone();
                tokio::spawn(async move { phone.reset_memory(erase, keys).await })
            };
            // Wait until the erase is running, then stop the desktop.
            let until = std::time::Instant::now() + Duration::from_secs(10);
            while keys.signed.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < until {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
            eprintln!("interrupted erase: save phone record");
            let record = phone.paired_record().await.unwrap();
            // Closing never waits for the stuck call.
            tokio::time::timeout(Duration::from_secs(5), desktop.close())
                .await
                .expect("close does not wait for running calls");
            eprintln!("interrupted erase: desktop closed");
            stuck.abort();
            let _ = stuck.await;
            // Retire the old client endpoint before restoring the same identity.
            // Merely aborting schedules cancellation and can leave it alive.
            phone.unpair().await;
            drop(phone);

            eprintln!("interrupted erase: restore desktop");
            let desktop = saved_desktop(&roots, &dir, &secret, Arc::new(store.clone())).await;
            let phone = PhoneLink::restore(identity, Some(record)).unwrap();
            eprintln!("interrupted erase: reconnect restored phone");
            phone.connect(false).await.unwrap();
            eprintln!("interrupted erase: replay original request");
            let error = phone.reset_memory(erase, keys.clone()).await.unwrap_err();
            assert!(
                matches!(&error, MobileError::Link { detail } if detail.contains("may or may not")),
                "{error:?}"
            );
            assert_eq!(keys.signed.load(Ordering::SeqCst), 1, "no second prompt");
            desktop.close().await;
            std::fs::remove_dir_all(&dir).unwrap();
        })
        .await;
    }
}
