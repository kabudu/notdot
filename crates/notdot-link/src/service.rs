//! Desktop-side execution of `link/1` calls, independent of the transport.
//!
//! - Idempotency: a call is identified by (device, [`RequestId`]) and bound
//!   to its operation's digest. A retry after a reconnect returns the
//!   recorded outcome of a committed (or cancelled) side effect, or joins
//!   the call still running; the side effect happens at most once. Refusals
//!   and read-only results are not recorded: they change nothing, so a retry
//!   simply runs again (e.g. after fetching a step-up challenge).
//! - Independence from the connection: calls run in their own task, so a
//!   dropped connection never abandons or cancels a half-done call.
//! - Cancellation: only an explicit cancel stops a call, and only before it
//!   commits. Commit and the final cancellation check share one lock.
//! - Backpressure: at most [`MAX_IN_FLIGHT`] calls per device; more get
//!   `Busy` (not recorded, so the same id can be retried).
//! - Isolation: every call acts on the device's own profile, chosen at
//!   pairing; scopes are checked on every call.
//!
//! The store is synthetic. In the desktop it becomes the existing notes,
//! settings and memory modules behind the same checks.
use crate::{
    DeviceId, LinkError,
    attest::AttestationPolicy,
    pairing::{DeviceRecord, Scope},
    protocol::{MAX_NOTE_BYTES, Op, OpError, RequestId, ServerFrame, StepUpProof},
    stepup::{StepUpDesk, StepUpState, op_digest},
};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::watch;

/// Calls in flight per device.
pub const MAX_IN_FLIGHT: usize = 8;
/// Recorded outcomes per device, oldest evicted first.
pub const MAX_RECORDED: usize = 256;
/// Longest synthetic task, in steps of [`TASK_STEP`].
pub const MAX_TASK_STEPS: u32 = 2_000;
pub const TASK_STEP: Duration = Duration::from_millis(5);
/// Notes kept per profile in the synthetic store.
pub const MAX_NOTES_PER_PROFILE: usize = 500;
/// ListNotes returns the newest notes that fit in this many bytes of text,
/// so the reply always fits one frame.
pub const MAX_LIST_BYTES: usize = 48 * 1024;

#[derive(Debug, Default, Clone)]
pub struct ProfileData {
    pub notes: Vec<(u64, String)>,
    next_note: u64,
    pub settings: BTreeMap<String, String>,
    pub revision: u64,
    pub memories: Vec<String>,
    pub tasks_completed: u64,
}

#[derive(Default)]
struct Calls {
    in_flight: HashMap<RequestId, InFlight>,
    recorded: HashMap<RequestId, ([u8; 32], ServerFrame)>,
    order: VecDeque<RequestId>,
}
struct InFlight {
    digest: [u8; 32],
    result: watch::Receiver<Option<ServerFrame>>,
    cancel: watch::Sender<bool>,
}
impl Calls {
    fn record(&mut self, id: RequestId, digest: [u8; 32], frame: ServerFrame) {
        if self.recorded.insert(id, (digest, frame)).is_none() {
            self.order.push_back(id);
        }
        while self.order.len() > MAX_RECORDED {
            if let Some(old) = self.order.pop_front() {
                self.recorded.remove(&old);
            }
        }
    }
}

struct State {
    profiles: HashMap<String, ProfileData>,
    calls: HashMap<DeviceId, Calls>,
    step_up: StepUpDesk,
    policy: AttestationPolicy,
}

pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

pub fn system_clock() -> Clock {
    Arc::new(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64)
    })
}

#[derive(Clone)]
pub struct Service {
    state: Arc<Mutex<State>>,
    clock: Clock,
}

fn failed(id: RequestId, error: OpError) -> ServerFrame {
    ServerFrame::Failed { id, error }
}

fn reused(id: RequestId) -> ServerFrame {
    failed(
        id,
        OpError::BadRequest {
            reason: "request id reused for a different operation".into(),
        },
    )
}

/// Operations whose committed outcome must be replayed, not repeated.
fn has_side_effect(op: &Op) -> bool {
    matches!(
        op,
        Op::AddNote { .. }
            | Op::StageSetting { .. }
            | Op::RunTask { .. }
            | Op::ResetMemory {}
            | Op::StepUpEnroll { .. }
    )
}

impl Service {
    pub fn new(desktop: DeviceId, clock: Clock) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                profiles: HashMap::new(),
                calls: HashMap::new(),
                step_up: StepUpDesk::new(desktop),
                policy: AttestationPolicy::default(),
            })),
            clock,
        }
    }

    /// Which platform evidence the desktop accepts for step-up keys. The
    /// default accepts none.
    pub fn set_attestation_policy(&self, policy: AttestationPolicy) {
        self.lock().policy = policy;
    }

    /// Install a freshly fetched Google attestation status list.
    pub fn set_android_status_list(&self, status: crate::attest::android::StatusList) {
        self.lock().policy.set_status_list(status);
    }

    /// A phone's enrolled step-up key and grant, for the owner's screens.
    pub fn step_up_state(&self, device: DeviceId) -> Option<StepUpState> {
        self.lock().step_up.state(device)
    }

    /// Owner action: allow an enrolled phone's sensitive requests until
    /// `until_ms`. The caller checks the phone's profile.
    pub fn grant_sensitive(&self, device: DeviceId, until_ms: u64) -> Result<(), crate::LinkError> {
        self.lock().step_up.grant(device, until_ms)
    }

    /// Forget a phone's step-up key and grant.
    pub fn forget_step_up(&self, device: DeviceId) {
        self.lock().step_up.forget(device);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        // A panic while holding the lock leaves plain data; keep serving.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Snapshot of a profile, for tests and the desktop UI.
    pub fn profile(&self, id: &str) -> ProfileData {
        self.lock().profiles.get(id).cloned().unwrap_or_default()
    }

    pub fn in_flight(&self, device: &DeviceId) -> usize {
        self.lock()
            .calls
            .get(device)
            .map_or(0, |calls| calls.in_flight.len())
    }

    /// Run (or rejoin) a call and wait for its final frame.
    pub async fn call(
        &self,
        device: &DeviceRecord,
        id: RequestId,
        op: Op,
        step_up: Option<StepUpProof>,
    ) -> ServerFrame {
        let digest = op_digest(&op);
        let mut result = {
            let mut state = self.lock();
            let calls = state.calls.entry(device.device).or_default();
            if let Some((recorded, frame)) = calls.recorded.get(&id) {
                return if *recorded == digest {
                    frame.clone()
                } else {
                    reused(id)
                };
            }
            if let Some(running) = calls.in_flight.get(&id) {
                if running.digest != digest {
                    return reused(id);
                }
                running.result.clone()
            } else {
                if calls.in_flight.len() >= MAX_IN_FLIGHT {
                    return failed(id, OpError::Busy);
                }
                let (done, result) = watch::channel(None);
                let (cancel, cancelled) = watch::channel(false);
                calls.in_flight.insert(
                    id,
                    InFlight {
                        digest,
                        result: result.clone(),
                        cancel,
                    },
                );
                let service = self.clone();
                let device = device.clone();
                tokio::spawn(async move {
                    let replay = has_side_effect(&op);
                    let frame = service.execute(&device, id, op, step_up, cancelled).await;
                    let mut state = service.lock();
                    let calls = state.calls.entry(device.device).or_default();
                    calls.in_flight.remove(&id);
                    if replay
                        && matches!(
                            frame,
                            ServerFrame::Done { .. } | ServerFrame::Cancelled { .. }
                        )
                    {
                        calls.record(id, digest, frame.clone());
                    }
                    let _ = done.send(Some(frame));
                });
                result
            }
        };
        match result.wait_for(Option::is_some).await {
            Ok(frame) => frame.clone().expect("waited for Some"),
            // The task always sends before dropping its sender.
            Err(_) => failed(
                id,
                OpError::BadRequest {
                    reason: "call ended without an outcome".into(),
                },
            ),
        }
    }

    /// Ask a running call to stop before it commits.
    pub fn cancel(&self, device: &DeviceId, id: RequestId) -> bool {
        let state = self.lock();
        match state
            .calls
            .get(device)
            .and_then(|calls| calls.in_flight.get(&id))
        {
            Some(running) => {
                let _ = running.cancel.send(true);
                true
            }
            None => false,
        }
    }

    async fn execute(
        &self,
        device: &DeviceRecord,
        id: RequestId,
        op: Op,
        step_up: Option<StepUpProof>,
        cancelled: watch::Receiver<bool>,
    ) -> ServerFrame {
        let now = (self.clock)();
        let profile = device.profile.id.clone();
        let need = |scope: Scope| device.allows(scope);
        let done = |result: serde_json::Value| ServerFrame::Done { id, result };
        match op {
            Op::Ping {} => done(serde_json::json!({"pong": true})),
            Op::AddNote { text } => {
                if !need(Scope::Notes) {
                    return failed(id, OpError::Forbidden);
                }
                if text.trim().is_empty() || text.len() > MAX_NOTE_BYTES {
                    return failed(
                        id,
                        OpError::BadRequest {
                            reason: "note length".into(),
                        },
                    );
                }
                let mut state = self.lock();
                let data = state.profiles.entry(profile).or_default();
                if data.notes.len() >= MAX_NOTES_PER_PROFILE {
                    return failed(
                        id,
                        OpError::BadRequest {
                            reason: "note limit reached".into(),
                        },
                    );
                }
                data.next_note += 1;
                let note = data.next_note;
                data.notes.push((note, text));
                done(serde_json::json!({"note": note}))
            }
            Op::ListNotes {} => {
                if !need(Scope::Notes) {
                    return failed(id, OpError::Forbidden);
                }
                let state = self.lock();
                let all = state
                    .profiles
                    .get(&profile)
                    .map(|data| data.notes.as_slice())
                    .unwrap_or_default();
                // Newest notes that fit, returned oldest first.
                let mut budget = MAX_LIST_BYTES;
                let mut notes: Vec<_> = all
                    .iter()
                    .rev()
                    .take_while(|(_, text)| {
                        let fits = text.len() <= budget;
                        budget = budget.saturating_sub(text.len());
                        fits
                    })
                    .cloned()
                    .collect();
                notes.reverse();
                let truncated = notes.len() < all.len();
                done(serde_json::json!({"notes": notes, "truncated": truncated}))
            }
            Op::ReadSettings {} => {
                if !need(Scope::ReadSettings) {
                    return failed(id, OpError::Forbidden);
                }
                let state = self.lock();
                let data = state.profiles.get(&profile).cloned().unwrap_or_default();
                done(serde_json::json!({"revision": data.revision, "settings": data.settings}))
            }
            Op::StageSetting {
                key,
                value,
                base_revision,
            } => {
                if !need(Scope::StageSettings) {
                    return failed(id, OpError::Forbidden);
                }
                if key.is_empty() || key.len() > 64 || value.len() > 256 {
                    return failed(
                        id,
                        OpError::BadRequest {
                            reason: "setting size".into(),
                        },
                    );
                }
                let mut state = self.lock();
                let data = state.profiles.entry(profile).or_default();
                if data.revision != base_revision {
                    return failed(
                        id,
                        OpError::Conflict {
                            revision: data.revision,
                        },
                    );
                }
                data.settings.insert(key, value);
                data.revision += 1;
                done(serde_json::json!({"revision": data.revision}))
            }
            Op::RunTask { label, steps } => {
                if !need(Scope::Tasks) {
                    return failed(id, OpError::Forbidden);
                }
                if steps > MAX_TASK_STEPS || label.len() > 128 {
                    return failed(
                        id,
                        OpError::BadRequest {
                            reason: "task too large".into(),
                        },
                    );
                }
                for _ in 0..steps {
                    if *cancelled.borrow() {
                        return ServerFrame::Cancelled { id };
                    }
                    tokio::time::sleep(TASK_STEP).await;
                }
                let mut state = self.lock();
                // Final check under the commit lock: cancel and commit
                // cannot both happen.
                if *cancelled.borrow() {
                    return ServerFrame::Cancelled { id };
                }
                let data = state.profiles.entry(profile).or_default();
                data.tasks_completed += 1;
                done(serde_json::json!({"label": label, "completed": data.tasks_completed}))
            }
            Op::StepUpChallenge { digest } => {
                if !need(Scope::Sensitive) {
                    return failed(id, OpError::Forbidden);
                }
                let mut guard = self.lock();
                let state = &mut *guard;
                if state.step_up.enrollment(device.device).is_none() {
                    return failed(id, OpError::StepUpNotEnrolled);
                }
                match state
                    .step_up
                    .challenge(device.device, digest, &state.policy, now)
                {
                    Ok(nonce) => done(serde_json::json!({"nonce": crate::b64::encode(&nonce)})),
                    Err(LinkError::Busy) => failed(id, OpError::Busy),
                    Err(LinkError::StepUpWithdrawn(reason)) => {
                        failed(id, OpError::AttestationRejected { reason })
                    }
                    Err(_) => failed(id, OpError::Forbidden),
                }
            }
            Op::StepUpEnrollChallenge {} => {
                if !need(Scope::Sensitive) {
                    return failed(id, OpError::Forbidden);
                }
                match self.lock().step_up.enroll_challenge(device.device, now) {
                    Ok(nonce) => done(serde_json::json!({"nonce": crate::b64::encode(&nonce)})),
                    Err(_) => failed(id, OpError::Busy),
                }
            }
            Op::StepUpEnroll { nonce, evidence } => {
                if !need(Scope::Sensitive) {
                    return failed(id, OpError::Forbidden);
                }
                // Verification is CPU-bound (RSA and ECDSA chains); keep it
                // off the async workers. The challenge is checked inside.
                let service = self.clone();
                let phone = device.device;
                let outcome = tokio::task::spawn_blocking(move || {
                    let mut guard = service.lock();
                    let state = &mut *guard;
                    state
                        .step_up
                        .enroll(phone, &nonce, &evidence, &state.policy, now)
                })
                .await;
                match outcome {
                    Ok(Ok(Ok(enrolled))) => done(serde_json::json!({
                        "protection": enrolled.protection,
                        "test_only": enrolled.test_only,
                    })),
                    Ok(Ok(Err(rejected))) => failed(
                        id,
                        OpError::AttestationRejected {
                            reason: rejected.to_string(),
                        },
                    ),
                    Ok(Err(LinkError::StepUpExpired)) => failed(id, OpError::StepUpExpired),
                    Ok(Err(_)) => failed(id, OpError::StepUpRejected),
                    Err(_) => failed(
                        id,
                        OpError::BadRequest {
                            reason: "enrolment stopped".into(),
                        },
                    ),
                }
            }
            Op::StepUpStatus {} => {
                if !need(Scope::Sensitive) {
                    return failed(id, OpError::Forbidden);
                }
                let state = self.lock().step_up.state(device.device);
                done(match state {
                    None => serde_json::json!({"enrolled": false}),
                    Some(state) => serde_json::json!({
                        "enrolled": true,
                        "protection": state.protection,
                        "test_only": state.test_only,
                        "development": state.development,
                        "key_sha256": crate::b64::encode(&state.key_sha256),
                        "granted": state.granted_until_ms.is_some_and(|until| now < until),
                    }),
                })
            }
            op @ Op::ResetMemory {} => {
                if !need(Scope::Sensitive) {
                    return failed(id, OpError::Forbidden);
                }
                let Some(proof) = step_up else {
                    return failed(id, OpError::NeedsStepUp);
                };
                let mut guard = self.lock();
                let state = &mut *guard;
                // Fail closed: any verification error refuses the action.
                // Verification (including a recheck of the enrolment against
                // the current policy and revocation list) and the effect
                // happen under one lock, so a proof is spent exactly when the
                // action commits.
                if let Err(error) = state.step_up.verify(
                    device.device,
                    &proof.nonce,
                    &op_digest(&op),
                    &proof,
                    &state.policy,
                    now,
                ) {
                    return failed(
                        id,
                        match error {
                            LinkError::StepUpExpired => OpError::StepUpExpired,
                            LinkError::Forbidden => OpError::Forbidden,
                            LinkError::StepUpWithdrawn(reason) => {
                                OpError::AttestationRejected { reason }
                            }
                            _ => OpError::StepUpRejected,
                        },
                    );
                }
                let data = state.profiles.entry(profile).or_default();
                let removed = data.memories.len();
                data.memories.clear();
                done(serde_json::json!({"removed": removed}))
            }
        }
    }

    /// Test and desktop helper: add a memory to a profile.
    pub fn remember(&self, profile: &str, memory: &str) {
        self.lock()
            .profiles
            .entry(profile.to_owned())
            .or_default()
            .memories
            .push(memory.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        attest::testing::TestRoots,
        pairing::{DeviceProfile, ProfileRef},
        stepup,
    };
    use p256::ecdsa::{Signature, SigningKey, signature::Signer};
    use std::sync::atomic::{AtomicU64, Ordering};

    const DESKTOP: DeviceId = DeviceId([1; 32]);

    fn record(device: u8, profile: &str, kind: DeviceProfile) -> DeviceRecord {
        DeviceRecord {
            device: DeviceId([device; 32]),
            name: "synthetic".into(),
            profile: ProfileRef {
                id: profile.into(),
                epoch: 1,
            },
            device_profile: kind,
            scopes: kind.scopes(),
            paired_ms: 0,
        }
    }

    const START: u64 = 1_790_000_000_000;

    fn service() -> (Service, Arc<AtomicU64>) {
        let now = Arc::new(AtomicU64::new(START));
        let clock = now.clone();
        (
            Service::new(DESKTOP, Arc::new(move || clock.load(Ordering::SeqCst))),
            now,
        )
    }

    fn note(text: &str) -> Op {
        Op::AddNote { text: text.into() }
    }

    #[tokio::test]
    async fn retries_return_the_recorded_outcome_without_repeating_effects() {
        let (service, _) = service();
        let owner = record(2, "owner", DeviceProfile::Owner);
        let id = RequestId(1);
        let first = service.call(&owner, id, note("milk"), None).await;
        let again = service.call(&owner, id, note("milk"), None).await;
        assert_eq!(first, again);
        assert_eq!(service.profile("owner").notes.len(), 1);
        // A different id is a different call.
        service.call(&owner, RequestId(2), note("milk"), None).await;
        assert_eq!(service.profile("owner").notes.len(), 2);
    }

    #[tokio::test]
    async fn a_retry_joins_a_running_call_and_a_dropped_caller_does_not_abort_it() {
        let (service, _) = service();
        let owner = record(2, "owner", DeviceProfile::Owner);
        let task = Op::RunTask {
            label: "sync".into(),
            steps: 20,
        };
        let id = RequestId(9);
        // The first caller goes away mid-call (connection lost).
        let first = tokio::time::timeout(
            Duration::from_millis(20),
            service.call(&owner, id, task.clone(), None),
        )
        .await;
        assert!(first.is_err());
        assert_eq!(service.in_flight(&owner.device), 1);
        let (a, b) = tokio::join!(
            service.call(&owner, id, task.clone(), None),
            service.call(&owner, id, task, None)
        );
        assert_eq!(a, b);
        assert!(matches!(a, ServerFrame::Done { .. }));
        assert_eq!(service.profile("owner").tasks_completed, 1);
    }

    #[tokio::test]
    async fn cancellation_stops_before_commit_and_is_recorded() {
        let (service, _) = service();
        let owner = record(2, "owner", DeviceProfile::Owner);
        let id = RequestId(3);
        let call = service.call(
            &owner,
            id,
            Op::RunTask {
                label: "long".into(),
                steps: 400,
            },
            None,
        );
        let cancel = async {
            tokio::time::sleep(Duration::from_millis(30)).await;
            assert!(service.cancel(&owner.device, id));
        };
        let (frame, ()) = tokio::join!(call, cancel);
        assert_eq!(frame, ServerFrame::Cancelled { id });
        assert_eq!(service.profile("owner").tasks_completed, 0);
        // Cancelling a finished call finds nothing and changes nothing; a
        // retry replays the cancellation instead of running the task.
        assert!(!service.cancel(&owner.device, id));
        let task = Op::RunTask {
            label: "long".into(),
            steps: 400,
        };
        assert_eq!(
            service.call(&owner, id, task, None).await,
            ServerFrame::Cancelled { id }
        );
        assert_eq!(service.profile("owner").tasks_completed, 0);
    }

    #[tokio::test]
    async fn in_flight_calls_are_bounded_per_device() {
        let (service, _) = service();
        let owner = record(2, "owner", DeviceProfile::Owner);
        let member = record(3, "ama", DeviceProfile::Member);
        let long = |n| {
            let service = service.clone();
            let owner = owner.clone();
            tokio::spawn(async move {
                service
                    .call(
                        &owner,
                        RequestId(100 + n),
                        Op::RunTask {
                            label: "x".into(),
                            steps: 40,
                        },
                        None,
                    )
                    .await
            })
        };
        let running: Vec<_> = (0..MAX_IN_FLIGHT as u128).map(long).collect();
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(service.in_flight(&owner.device), MAX_IN_FLIGHT);
        let busy = service.call(&owner, RequestId(1), Op::Ping {}, None).await;
        assert_eq!(busy, failed(RequestId(1), OpError::Busy));
        // Another device is not starved by this one.
        assert!(matches!(
            service.call(&member, RequestId(1), Op::Ping {}, None).await,
            ServerFrame::Done { .. }
        ));
        for task in running {
            assert!(matches!(task.await.unwrap(), ServerFrame::Done { .. }));
        }
        // Busy was not recorded: the same id now runs.
        assert!(matches!(
            service.call(&owner, RequestId(1), Op::Ping {}, None).await,
            ServerFrame::Done { .. }
        ));
    }

    #[tokio::test]
    async fn members_see_only_their_own_data_and_cannot_stage_settings() {
        let (service, _) = service();
        let owner = record(2, "owner", DeviceProfile::Owner);
        let member = record(3, "ama", DeviceProfile::Member);
        service
            .call(&owner, RequestId(1), note("owner secret"), None)
            .await;
        service
            .call(&member, RequestId(1), note("ama's list"), None)
            .await;
        let ServerFrame::Done { result, .. } = service
            .call(&member, RequestId(2), Op::ListNotes {}, None)
            .await
        else {
            panic!()
        };
        assert_eq!(result["notes"].as_array().unwrap().len(), 1);
        assert!(!result.to_string().contains("owner secret"));
        let stage = Op::StageSetting {
            key: "hue".into(),
            value: "42".into(),
            base_revision: 0,
        };
        assert_eq!(
            service.call(&member, RequestId(3), stage, None).await,
            failed(RequestId(3), OpError::Forbidden)
        );
        assert_eq!(service.profile("owner").settings.len(), 0);
    }

    #[tokio::test]
    async fn settings_use_compare_and_set_revisions() {
        let (service, _) = service();
        let owner = record(2, "owner", DeviceProfile::Owner);
        let stage = |value: &str, base| Op::StageSetting {
            key: "hue".into(),
            value: value.into(),
            base_revision: base,
        };
        assert!(matches!(
            service
                .call(&owner, RequestId(1), stage("42", 0), None)
                .await,
            ServerFrame::Done { .. }
        ));
        assert_eq!(
            service
                .call(&owner, RequestId(2), stage("90", 0), None)
                .await,
            failed(RequestId(2), OpError::Conflict { revision: 1 })
        );
        assert_eq!(service.profile("owner").settings["hue"], "42");
    }

    fn nonce_of(frame: ServerFrame) -> [u8; 32] {
        let ServerFrame::Done { result, .. } = frame else {
            panic!("no challenge: {frame:?}")
        };
        stepup::challenge_nonce(&result).unwrap()
    }

    /// Enrol an attested (test-root) Android key through the service.
    async fn enrol(service: &Service, phone: &DeviceRecord, roots: &TestRoots) -> SigningKey {
        let nonce = nonce_of(
            service
                .call(
                    phone,
                    RequestId::random(),
                    Op::StepUpEnrollChallenge {},
                    None,
                )
                .await,
        );
        let challenge = crate::attest::attestation_challenge(DESKTOP, phone.device, &nonce);
        let (key, evidence) = roots.android_evidence(challenge);
        let frame = service
            .call(
                phone,
                RequestId::random(),
                Op::StepUpEnroll { nonce, evidence },
                None,
            )
            .await;
        let ServerFrame::Done { result, .. } = frame else {
            panic!("enrolment refused: {frame:?}")
        };
        assert_eq!(result["protection"], "android_trusted_environment");
        assert_eq!(result["test_only"], true);
        key
    }

    fn sign(key: &SigningKey, phone: DeviceId, nonce: [u8; 32], digest: [u8; 32]) -> StepUpProof {
        let signature: Signature = key.sign(&stepup::message(DESKTOP, phone, &nonce, &digest));
        StepUpProof {
            nonce,
            signature: signature.to_der().as_bytes().to_vec(),
            assertion: None,
        }
    }

    #[tokio::test]
    async fn enrolment_needs_attested_evidence_for_this_challenge() {
        let (service, _) = service();
        let roots = TestRoots::new(START);
        let owner = record(2, "owner", DeviceProfile::Owner);
        let member = record(3, "ama", DeviceProfile::Member);
        // Members cannot enrol.
        assert_eq!(
            service
                .call(&member, RequestId(1), Op::StepUpEnrollChallenge {}, None)
                .await,
            failed(RequestId(1), OpError::Forbidden)
        );
        // The default policy accepts no evidence, even well-formed.
        let nonce = nonce_of(
            service
                .call(&owner, RequestId(2), Op::StepUpEnrollChallenge {}, None)
                .await,
        );
        let (_, evidence) = roots.android_evidence(crate::attest::attestation_challenge(
            DESKTOP,
            owner.device,
            &nonce,
        ));
        let frame = service
            .call(
                &owner,
                RequestId(3),
                Op::StepUpEnroll {
                    nonce,
                    evidence: evidence.clone(),
                },
                None,
            )
            .await;
        assert!(
            matches!(&frame, ServerFrame::Failed { error: OpError::AttestationRejected { reason }, .. } if reason.contains("not set up")),
            "{frame:?}"
        );
        assert_eq!(service.step_up_state(owner.device), None);
        // Nor can the spent challenge be reused once a policy exists.
        service.set_attestation_policy(roots.policy(START));
        assert_eq!(
            service
                .call(
                    &owner,
                    RequestId(4),
                    Op::StepUpEnroll { nonce, evidence },
                    None
                )
                .await,
            failed(RequestId(4), OpError::StepUpRejected)
        );
        enrol(&service, &owner, &roots).await;
        let state = service.step_up_state(owner.device).unwrap();
        assert_eq!(
            state.protection,
            crate::attest::Protection::AndroidTrustedEnvironment
        );
        assert!(state.test_only);
        assert_eq!(state.granted_until_ms, None);
        // The phone can read its own status.
        let ServerFrame::Done { result, .. } = service
            .call(&owner, RequestId(5), Op::StepUpStatus {}, None)
            .await
        else {
            panic!()
        };
        assert_eq!(result["enrolled"], true);
        assert_eq!(result["granted"], false);
    }

    #[tokio::test]
    async fn sensitive_actions_need_enrolment_grant_and_fresh_operation_bound_step_up() {
        let (service, now) = service();
        let roots = TestRoots::new(START);
        service.set_attestation_policy(roots.policy(START));
        let owner = record(2, "owner", DeviceProfile::Owner);
        let member = record(3, "ama", DeviceProfile::Member);
        service.remember("owner", "likes tea");
        let digest = op_digest(&Op::ResetMemory {});
        // Not enrolled.
        assert_eq!(
            service
                .call(&owner, RequestId(1), Op::StepUpChallenge { digest }, None)
                .await,
            failed(RequestId(1), OpError::StepUpNotEnrolled)
        );
        let key = enrol(&service, &owner, &roots).await;
        // Enrolled but no grant: the phone cannot even ask for a challenge.
        assert_eq!(
            service
                .call(&owner, RequestId(2), Op::StepUpChallenge { digest }, None)
                .await,
            failed(RequestId(2), OpError::Forbidden)
        );
        service
            .grant_sensitive(owner.device, START + 60_000)
            .unwrap();
        assert_eq!(
            service
                .call(&owner, RequestId(3), Op::ResetMemory {}, None)
                .await,
            failed(RequestId(3), OpError::NeedsStepUp)
        );
        let challenge = |n| {
            let service = service.clone();
            let owner = owner.clone();
            async move {
                nonce_of(
                    service
                        .call(&owner, RequestId(n), Op::StepUpChallenge { digest }, None)
                        .await,
                )
            }
        };
        // Signed for another operation.
        let nonce = challenge(4).await;
        assert_eq!(
            service
                .call(
                    &owner,
                    RequestId(5),
                    Op::ResetMemory {},
                    Some(sign(&key, owner.device, nonce, [0; 32]))
                )
                .await,
            failed(RequestId(5), OpError::StepUpRejected)
        );
        // Signed with an unenrolled key.
        let nonce = challenge(6).await;
        let other = crate::attest::testing::random_key();
        assert_eq!(
            service
                .call(
                    &owner,
                    RequestId(7),
                    Op::ResetMemory {},
                    Some(sign(&other, owner.device, nonce, digest))
                )
                .await,
            failed(RequestId(7), OpError::StepUpRejected)
        );
        assert_eq!(service.profile("owner").memories.len(), 1);
        let nonce = challenge(8).await;
        let proof = sign(&key, owner.device, nonce, digest);
        assert!(matches!(
            service
                .call(
                    &owner,
                    RequestId(9),
                    Op::ResetMemory {},
                    Some(proof.clone())
                )
                .await,
            ServerFrame::Done { .. }
        ));
        assert!(service.profile("owner").memories.is_empty());
        // Replaying the proof under a new id fails; under the same id the
        // recorded outcome comes back and nothing runs twice.
        service.remember("owner", "likes coffee");
        assert_eq!(
            service
                .call(
                    &owner,
                    RequestId(10),
                    Op::ResetMemory {},
                    Some(proof.clone())
                )
                .await,
            failed(RequestId(10), OpError::StepUpRejected)
        );
        assert!(matches!(
            service
                .call(&owner, RequestId(9), Op::ResetMemory {}, None)
                .await,
            ServerFrame::Done { .. }
        ));
        assert_eq!(service.profile("owner").memories.len(), 1);
        assert_eq!(
            service
                .call(&member, RequestId(11), Op::ResetMemory {}, Some(proof))
                .await,
            failed(RequestId(11), OpError::Forbidden)
        );
        // An expired challenge says so.
        let nonce = challenge(12).await;
        now.fetch_add(stepup::CHALLENGE_TTL_MS, Ordering::SeqCst);
        assert_eq!(
            service
                .call(
                    &owner,
                    RequestId(13),
                    Op::ResetMemory {},
                    Some(sign(&key, owner.device, nonce, digest))
                )
                .await,
            failed(RequestId(13), OpError::StepUpExpired)
        );
        // Re-enrolling ends the grant.
        enrol(&service, &owner, &roots).await;
        assert_eq!(
            service
                .call(&owner, RequestId(14), Op::StepUpChallenge { digest }, None)
                .await,
            failed(RequestId(14), OpError::Forbidden)
        );
        assert_eq!(service.profile("owner").memories.len(), 1);
    }

    #[tokio::test]
    async fn a_request_id_is_bound_to_its_operation_and_refusals_are_not_replayed() {
        let (service, _) = service();
        let roots = TestRoots::new(START);
        service.set_attestation_policy(roots.policy(START));
        let owner = record(2, "owner", DeviceProfile::Owner);
        let id = RequestId(1);
        assert!(matches!(
            service.call(&owner, id, note("milk"), None).await,
            ServerFrame::Done { .. }
        ));
        // Same id, different operation: refused, nothing recorded or run.
        assert!(matches!(
            service.call(&owner, id, note("eggs"), None).await,
            ServerFrame::Failed {
                error: OpError::BadRequest { .. },
                ..
            }
        ));
        assert_eq!(service.profile("owner").notes.len(), 1);
        // NeedsStepUp is not recorded: the same id succeeds once a proof
        // is attached (the natural client flow).
        let key = enrol(&service, &owner, &roots).await;
        service
            .grant_sensitive(owner.device, START + 60_000)
            .unwrap();
        let reset = RequestId(2);
        assert_eq!(
            service.call(&owner, reset, Op::ResetMemory {}, None).await,
            failed(reset, OpError::NeedsStepUp)
        );
        let digest = op_digest(&Op::ResetMemory {});
        let nonce = nonce_of(
            service
                .call(&owner, RequestId(3), Op::StepUpChallenge { digest }, None)
                .await,
        );
        let proof = sign(&key, owner.device, nonce, digest);
        assert!(matches!(
            service
                .call(&owner, reset, Op::ResetMemory {}, Some(proof))
                .await,
            ServerFrame::Done { .. }
        ));
    }

    #[tokio::test]
    async fn notes_are_bounded_and_listing_always_fits_a_frame() {
        let (service, _) = service();
        let owner = record(2, "owner", DeviceProfile::Owner);
        let big = "n".repeat(MAX_NOTE_BYTES);
        for n in 0..MAX_NOTES_PER_PROFILE as u128 {
            service.call(&owner, RequestId(n), note(&big), None).await;
        }
        assert!(matches!(
            service
                .call(&owner, RequestId(9_999), note("one more"), None)
                .await,
            ServerFrame::Failed {
                error: OpError::BadRequest { .. },
                ..
            }
        ));
        let ServerFrame::Done { result, .. } = service
            .call(&owner, RequestId(10_000), Op::ListNotes {}, None)
            .await
        else {
            panic!()
        };
        assert_eq!(result["truncated"], true);
        let frame = serde_json::to_vec(&ServerFrame::Done {
            id: RequestId(0),
            result,
        })
        .unwrap();
        assert!(frame.len() < crate::protocol::MAX_FRAME, "{}", frame.len());
    }

    #[tokio::test]
    async fn recorded_outcomes_are_bounded() {
        let (service, _) = service();
        let owner = record(2, "owner", DeviceProfile::Owner);
        for n in 0..(MAX_RECORDED as u128 + 10) {
            service.call(&owner, RequestId(n), note("x"), None).await;
        }
        let state = service.lock();
        let calls = &state.calls[&owner.device];
        assert_eq!(calls.recorded.len(), MAX_RECORDED);
        assert!(!calls.recorded.contains_key(&RequestId(0)));
    }
}
