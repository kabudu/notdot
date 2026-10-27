use super::{AppState, backend};
use notdot_voice::{
    VoiceError,
    quota::{Ledger, Status},
    usage::UsageReport,
};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::State;
use tokio::sync::Mutex;
const PERIOD: u64 = 15 * 60;
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Record {
    ledger: Ledger,
    next_speech_attempt: u64,
    speech_error: Option<String>,
}
pub struct Monitor {
    record: Mutex<Record>,
    refresh: Mutex<()>,
    announce: Mutex<()>,
    path: PathBuf,
    generation: AtomicU64,
    dirty: AtomicBool,
}
impl Monitor {
    pub fn new(path: PathBuf) -> Self {
        let record = std::fs::metadata(&path)
            .ok()
            .filter(|m| m.len() <= 256 * 1024)
            .and_then(|_| std::fs::read(&path).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            record: Mutex::new(record),
            refresh: Mutex::new(()),
            announce: Mutex::new(()),
            path,
            generation: AtomicU64::new(0),
            dirty: AtomicBool::new(false),
        }
    }
    async fn save(&self, record: &Record) -> Result<(), String> {
        let bytes = serde_json::to_vec(record).map_err(|e| e.to_string())?;
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            use std::io::Write;
            let temp = path.with_extension("tmp");
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            std::fs::rename(temp, path)
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("Could not save usage warning history: {e}"))
    }
    async fn deliver<F, Fut>(&self, provider: &str, id: &str, speak: F) -> Result<(), String>
    where
        F: FnOnce(String) -> Fut,
        Fut: std::future::Future<Output = Result<(), String>>,
    {
        let _announce = self
            .announce
            .try_lock()
            .map_err(|_| "Usage announcement is already playing")?;
        let (text, prior) = {
            let mut record = self.record.lock().await;
            if record.next_speech_attempt > now() {
                return Err("Local voice alert is waiting to retry".into());
            }
            let prior = record.ledger.clone();
            let text = record
                .ledger
                .mark(provider, id, now())
                .ok_or("This usage alert is no longer current")?;
            if let Err(e) = self.save(&record).await {
                record.ledger.restore_history(&prior);
                return Err(e);
            }
            (text, prior)
        };
        if let Err(error) = speak(text).await {
            let mut record = self.record.lock().await;
            record.ledger.restore_history(&prior);
            record.next_speech_attempt = now() + PERIOD;
            record.speech_error = Some(format!("Local voice alert unavailable: {error}"));
            self.save(&record).await?;
            return Err(error);
        }
        let mut record = self.record.lock().await;
        record.next_speech_attempt = 0;
        record.speech_error = None;
        self.save(&record).await
    }
    pub async fn status(&self, provider: &str) -> Status {
        let record = self.record.lock().await;
        let mut status = record.ledger.status(provider, now());
        if record.next_speech_attempt > now() {
            status.notice_id = None;
        }
        if let Some(error) = &record.speech_error {
            status.message = format!("{}; {error}", status.message);
        }
        status
    }
    pub async fn invalidate(&self) -> Result<(), String> {
        self.generation.fetch_add(1, Ordering::SeqCst);
        let mut record = self.record.lock().await;
        record.ledger.cache = None;
        record.next_speech_attempt = 0;
        record.speech_error = None;
        self.dirty.store(true, Ordering::SeqCst);
        self.save(&record).await
    }
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }
    async fn result<T>(
        &self,
        provider: &str,
        generation: u64,
        operation: &str,
        result: Result<T, VoiceError>,
    ) -> Result<T, String> {
        let mut record = self.record.lock().await;
        if generation != self.generation() {
            return result.map_err(|e| e.to_string());
        }
        let had_fault = record
            .ledger
            .cache
            .as_ref()
            .is_some_and(|c| c.fault.is_some());
        match &result {
            Ok(_) => {
                if record
                    .ledger
                    .cache
                    .as_ref()
                    .and_then(|c| c.fault.as_ref())
                    .is_some_and(|f| f.operation == operation)
                {
                    record.ledger.recovered(provider);
                }
            }
            Err(error) => record
                .ledger
                .fault(provider, error.kind, &error.to_string(), now()),
        }
        if result.is_err()
            && let Some(f) = record.ledger.cache.as_mut().and_then(|c| c.fault.as_mut())
        {
            f.operation = operation.into();
        }
        self.dirty.store(true, Ordering::SeqCst);
        if result.is_ok() && !had_fault {
            return result.map_err(|e| e.to_string());
        }
        // A history failure is visible without replacing the underlying provider error.
        if let Err(e) = self.save(&record).await {
            record.speech_error = Some(e);
        }
        result.map_err(|e| e.to_string())
    }
}
pub async fn finish<T>(
    state: &AppState,
    provider: &str,
    generation: u64,
    operation: &str,
    result: Result<T, VoiceError>,
) -> Result<T, String> {
    let selected = state.snapshot.lock().await.settings.voice_provider == provider;
    if selected {
        state
            .quota
            .result(provider, generation, operation, result)
            .await
    } else {
        result.map_err(|e| e.to_string())
    }
}

fn validate(report: &UsageReport) -> Result<(), String> {
    if report.account.is_empty()
        || report.account.len() > 300
        || report.meters.len() > 8
        || report.note.len() > 2000
        || report
            .meters
            .iter()
            .any(|m| m.id.len() > 100 || m.label.len() > 200 || m.unit.len() > 100)
    {
        return Err("Usage unavailable: provider report exceeded its bounds".into());
    }
    Ok(())
}
async fn refresh(state: &AppState, force: bool) -> Result<(), String> {
    let monitor = &state.quota;
    let _refresh = monitor
        .refresh
        .try_lock()
        .map_err(|_| "Usage check is already in progress")?;
    let (provider, ready) = {
        let s = state.snapshot.lock().await;
        (
            s.settings.voice_provider.clone(),
            s.key_configured && !s.key_checking,
        )
    };
    if !ready {
        return Err("Usage unavailable: save or authorize the provider key first".into());
    }
    let generation = monitor.generation.load(Ordering::SeqCst);
    {
        let mut r = monitor.record.lock().await;
        if force {
            r.next_speech_attempt = 0;
        }
        if !force && let Some(c) = r.ledger.cache.as_ref().filter(|c| c.provider == provider) {
            let elapsed = now().saturating_sub(c.last_checked);
            let reset_due = c.report.as_ref().is_some_and(|r| {
                r.meters.iter().any(|m| {
                    m.reset_unix
                        .is_some_and(|t| t <= now() && c.last_checked < t)
                })
            });
            let period = if monitor.dirty.load(Ordering::SeqCst) {
                60
            } else {
                PERIOD
            };
            if now() >= c.last_checked && elapsed < period && !reset_due {
                return Ok(());
            }
        }
    }
    let _permit = state
        .voice_gate
        .try_acquire()
        .map_err(|_| "Voice service is busy; usage will refresh later")?;
    let result = async {
        let report = backend(state, Some(provider.clone()))
            .await?
            .usage()
            .await
            .map_err(|e| format!("Usage unavailable: {e}"))?;
        validate(&report)?;
        Ok::<_, String>(report)
    }
    .await;
    let selected = state.snapshot.lock().await.settings.voice_provider == provider;
    let mut record = monitor.record.lock().await;
    if monitor.generation.load(Ordering::SeqCst) != generation {
        return Ok(());
    }
    if !selected {
        return Ok(());
    }
    let error = result.as_ref().err().cloned();
    record.ledger.observe(&provider, result, now());
    monitor.dirty.store(false, Ordering::SeqCst);
    monitor.save(&record).await?;
    if let Some(error) = error {
        Err(error)
    } else {
        Ok(())
    }
}
#[tauri::command]
pub async fn refresh_usage(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    refresh(&state, true).await
}
#[tauri::command]
pub async fn announce_usage(state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    if id.len() > 1024 {
        return Err("Invalid usage alert".into());
    }
    let monitor = &state.quota;
    let _turn = state
        .send_gate
        .try_lock()
        .map_err(|_| "Wait for the current submission")?;
    let _voice = state
        .voice_gate
        .try_acquire_many(4)
        .map_err(|_| "Wait for voice processing")?;
    let provider = {
        let s = state.snapshot.lock().await;
        if s.active_turn.is_some() || s.phase != "idle" {
            return Err("Wait for the current response".into());
        }
        s.settings.voice_provider.clone()
    };
    monitor
        .deliver(&provider, &id, |text| async move {
            super::system_speech::speak(&text).await
        })
        .await
}

pub fn spawn(state: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        let mut ticks = tokio::time::interval(Duration::from_secs(15));
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticks.tick().await;
            let _ = refresh(&state, false).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use notdot_voice::usage::{Overage, UsageMeter};
    fn report() -> UsageReport {
        UsageReport {
            account: "synthetic-account".into(),
            note: "No STT balance reported".into(),
            meters: vec![UsageMeter {
                id: "subscription".into(),
                label: "Subscription".into(),
                unit: "credits".into(),
                used: 800,
                limit: 1000,
                reset_unix: Some(now() + 10000),
                overage: Overage::Unknown,
            }],
        }
    }
    #[tokio::test]
    async fn stale_credentials_and_unrelated_voice_success_cannot_clear_or_create_faults() {
        use notdot_voice::VoiceErrorKind;
        let dir = tempfile::tempdir().unwrap();
        let monitor = Monitor::new(dir.path().join("usage.json"));
        let generation = monitor.generation();
        monitor
            .result::<()>(
                "elevenlabs",
                generation,
                "tts",
                Err(VoiceError::new(VoiceErrorKind::Quota, "quota")),
            )
            .await
            .unwrap_err();
        monitor
            .result("elevenlabs", generation, "stt", Ok(()))
            .await
            .unwrap();
        assert!(monitor.status("elevenlabs").await.fault.is_some());
        monitor
            .result("elevenlabs", generation, "tts", Ok(()))
            .await
            .unwrap();
        assert!(monitor.status("elevenlabs").await.fault.is_none());
        monitor.invalidate().await.unwrap();
        monitor
            .result::<()>(
                "elevenlabs",
                generation,
                "tts",
                Err(VoiceError::new(VoiceErrorKind::Quota, "old key")),
            )
            .await
            .unwrap_err();
        assert!(monitor.status("elevenlabs").await.fault.is_none());
    }
    #[tokio::test]
    async fn local_alert_delivery_persists_and_does_not_repeat_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.json");
        let monitor = Monitor::new(path.clone());
        monitor
            .record
            .lock()
            .await
            .ledger
            .observe("elevenlabs", Ok(report()), now());
        let id = monitor.status("elevenlabs").await.notice_id.unwrap();
        monitor
            .deliver("elevenlabs", &id, |text| async move {
                assert!(text.contains("80 percent"));
                Ok(())
            })
            .await
            .unwrap();
        monitor
            .deliver("elevenlabs", &id, |_| async {
                panic!("duplicate local speech")
            })
            .await
            .unwrap_err();
        let restored = Monitor::new(path);
        let status = restored.status("elevenlabs").await;
        assert_eq!(status.level, "warning");
        assert!(status.notice_id.is_none());
        assert_eq!(status.report.unwrap().account, "");
    }
    #[tokio::test]
    async fn missing_local_voice_preserves_warning_with_persisted_backoff() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.json");
        let monitor = Monitor::new(path.clone());
        monitor
            .record
            .lock()
            .await
            .ledger
            .observe("elevenlabs", Ok(report()), now());
        let id = monitor.status("elevenlabs").await.notice_id.unwrap();
        monitor
            .deliver("elevenlabs", &id, |_| async {
                Err("System voice unavailable".into())
            })
            .await
            .unwrap_err();
        let restored = Monitor::new(path);
        let status = restored.status("elevenlabs").await;
        assert!(status.notice_id.is_none());
        assert!(status.message.contains("System voice unavailable"));
        let record = restored.record.lock().await;
        assert!(record.ledger.notice("elevenlabs", now()).is_some());
        assert!(record.next_speech_attempt > now());
    }
}
