//! Desktop-side execution of `link/1` calls, independent of the transport.
//!
//! - Idempotency: a call is identified by (device, [`RequestId`]). A retry
//!   after a reconnect returns the recorded outcome, or joins the call still
//!   running; the side effect happens at most once.
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
    pairing::{DeviceRecord, Scope},
    protocol::{MAX_NOTE_BYTES, Op, OpError, RequestId, ServerFrame, StepUpProof},
    stepup::{StepUpDesk, op_digest},
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
    in_flight: HashMap<RequestId, (watch::Receiver<Option<ServerFrame>>, watch::Sender<bool>)>,
    recorded: HashMap<RequestId, ServerFrame>,
    order: VecDeque<RequestId>,
}
impl Calls {
    fn record(&mut self, id: RequestId, frame: ServerFrame) {
        if self.recorded.insert(id, frame).is_none() {
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

impl Service {
    pub fn new(desktop: DeviceId, clock: Clock) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                profiles: HashMap::new(),
                calls: HashMap::new(),
                step_up: StepUpDesk::new(desktop),
            })),
            clock,
        }
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
        let mut result = {
            let mut state = self.lock();
            let calls = state.calls.entry(device.device).or_default();
            if let Some(frame) = calls.recorded.get(&id) {
                return frame.clone();
            }
            if let Some((result, _)) = calls.in_flight.get(&id) {
                result.clone()
            } else {
                if calls.in_flight.len() >= MAX_IN_FLIGHT {
                    return failed(id, OpError::Busy);
                }
                let (done, result) = watch::channel(None);
                let (cancel, cancelled) = watch::channel(false);
                calls.in_flight.insert(id, (result.clone(), cancel));
                let service = self.clone();
                let device = device.clone();
                tokio::spawn(async move {
                    let frame = service.execute(&device, id, op, step_up, cancelled).await;
                    let mut state = service.lock();
                    let calls = state.calls.entry(device.device).or_default();
                    calls.in_flight.remove(&id);
                    calls.record(id, frame.clone());
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
            Some((_, cancel)) => {
                let _ = cancel.send(true);
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
        let need = |scope: Scope| device.allows(scope, now);
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
                let notes: Vec<_> = state
                    .profiles
                    .get(&profile)
                    .map(|data| data.notes.clone())
                    .unwrap_or_default();
                done(serde_json::json!({"notes": notes}))
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
                match self.lock().step_up.challenge(device.device, digest, now) {
                    Ok(nonce) => done(serde_json::json!({"nonce": crate::b64::encode(&nonce)})),
                    Err(_) => failed(id, OpError::Busy),
                }
            }
            op @ Op::ResetMemory {} => {
                if !need(Scope::Sensitive) {
                    return failed(id, OpError::Forbidden);
                }
                let Some(proof) = step_up else {
                    return failed(id, OpError::NeedsStepUp);
                };
                let mut state = self.lock();
                if let Err(LinkError::StepUpRejected) = state.step_up.verify(
                    device.device,
                    &device.step_up_key,
                    &proof.nonce,
                    &op_digest(&op),
                    &proof.signature,
                    now,
                ) {
                    return failed(id, OpError::StepUpRejected);
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
        pairing::{DeviceProfile, ProfileRef},
        stepup,
    };
    use p256::ecdsa::{Signature, signature::Signer};
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
            step_up_key: stepup::tests::public(&stepup::tests::key()),
            sensitive_until_ms: None,
            paired_ms: 0,
        }
    }

    fn service() -> (Service, Arc<AtomicU64>) {
        let now = Arc::new(AtomicU64::new(1_000));
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
        // Cancelling a finished call finds nothing and changes nothing.
        assert!(!service.cancel(&owner.device, id));
        assert_eq!(
            service.call(&owner, id, Op::Ping {}, None).await,
            ServerFrame::Cancelled { id }
        );
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

    #[tokio::test]
    async fn sensitive_actions_need_grant_and_fresh_operation_bound_step_up() {
        let (service, _) = service();
        let mut owner = record(2, "owner", DeviceProfile::Owner);
        let member = record(3, "ama", DeviceProfile::Member);
        service.remember("owner", "likes tea");
        let digest = op_digest(&Op::ResetMemory {});
        // No grant yet: the phone cannot even ask for a challenge.
        assert_eq!(
            service
                .call(&owner, RequestId(1), Op::StepUpChallenge { digest }, None)
                .await,
            failed(RequestId(1), OpError::Forbidden)
        );
        owner.sensitive_until_ms = Some(10_000);
        assert_eq!(
            service
                .call(&owner, RequestId(2), Op::ResetMemory {}, None)
                .await,
            failed(RequestId(2), OpError::NeedsStepUp)
        );
        let sign = |nonce: [u8; 32], digest: [u8; 32]| {
            let signature: Signature =
                stepup::tests::key().sign(&stepup::message(DESKTOP, owner.device, &nonce, &digest));
            StepUpProof {
                nonce,
                signature: signature.to_der().as_bytes().to_vec(),
            }
        };
        let challenge = |n| {
            let service = service.clone();
            let owner = owner.clone();
            async move {
                let ServerFrame::Done { result, .. } = service
                    .call(&owner, RequestId(n), Op::StepUpChallenge { digest }, None)
                    .await
                else {
                    panic!()
                };
                let nonce: [u8; 32] = crate::b64::decode(result["nonce"].as_str().unwrap())
                    .unwrap()
                    .try_into()
                    .unwrap();
                nonce
            }
        };
        // Signed for another operation.
        let nonce = challenge(3).await;
        assert_eq!(
            service
                .call(
                    &owner,
                    RequestId(4),
                    Op::ResetMemory {},
                    Some(sign(nonce, [0; 32]))
                )
                .await,
            failed(RequestId(4), OpError::StepUpRejected)
        );
        let nonce = challenge(5).await;
        let proof = sign(nonce, digest);
        assert!(matches!(
            service
                .call(
                    &owner,
                    RequestId(6),
                    Op::ResetMemory {},
                    Some(proof.clone())
                )
                .await,
            ServerFrame::Done { .. }
        ));
        assert!(service.profile("owner").memories.is_empty());
        // Replaying the proof under a new id fails.
        assert_eq!(
            service
                .call(
                    &owner,
                    RequestId(7),
                    Op::ResetMemory {},
                    Some(proof.clone())
                )
                .await,
            failed(RequestId(7), OpError::StepUpRejected)
        );
        assert_eq!(
            service
                .call(&member, RequestId(8), Op::ResetMemory {}, Some(proof))
                .await,
            failed(RequestId(8), OpError::Forbidden)
        );
    }

    #[tokio::test]
    async fn recorded_outcomes_are_bounded() {
        let (service, _) = service();
        let owner = record(2, "owner", DeviceProfile::Owner);
        for n in 0..(MAX_RECORDED as u128 + 10) {
            service.call(&owner, RequestId(n), Op::Ping {}, None).await;
        }
        let state = service.lock();
        let calls = &state.calls[&owner.device];
        assert_eq!(calls.recorded.len(), MAX_RECORDED);
        assert!(!calls.recorded.contains_key(&RequestId(0)));
    }
}
