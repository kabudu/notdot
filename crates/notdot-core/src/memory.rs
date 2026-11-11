// Sled shares its flush pool across databases. Parallel temporary-store tests
// can starve that pool; the app owns one ledger. Serialize only these store
// fixtures while retaining all assertions and per-ledger concurrency checks.
#[cfg(test)]
static TEST_STORE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
mod learning;
pub use learning::*;
use mnemara::{
    CompactionRequest, DeleteRequest, EngineConfig, ExportRequest, MemoryHistoricalState,
    MemoryQualityState, MemoryRecord, MemoryRecordKind, MemoryScope, MemoryStore, MemoryTrustLevel,
    RecallFilters, RecallQuery, SledMemoryStore, SledStoreConfig, StoreStatsRequest,
    SynthesisRequest, UpsertRequest,
};
use std::{
    collections::BTreeMap,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
pub const MAX_RECORDS: usize = 2000;
/// Metadata that describes a memory itself. Chat and turn identifiers stay on
/// this computer.
pub const BACKUP_METADATA: &[&str] = &[
    "origin",
    "semantic_key",
    "evidence",
    "learning_state",
    "confidence",
];
/// One memory in an encrypted backup.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupRecord {
    pub id: String,
    pub summary: bool,
    pub content: String,
    pub metadata: BTreeMap<String, String>,
    pub created_unix_ms: u64,
    pub updated_unix_ms: u64,
    /// IDs of other records in the same backup this one replaces or summarises.
    pub sources: Vec<String>,
}
pub struct Memory {
    policy: tokio::sync::watch::Sender<LearningPolicy>,
    store: SledMemoryStore,
    mutations: tokio::sync::Mutex<()>,
    revision: std::sync::atomic::AtomicU64,
    maintained: std::sync::atomic::AtomicU64,
    semantic_reviewed: std::sync::atomic::AtomicU64,
}
fn scope() -> MemoryScope {
    MemoryScope {
        tenant_id: "local".into(),
        namespace: "notdot".into(),
        actor_id: "user".into(),
        source: "explicit-user-note".into(),
        trust_level: MemoryTrustLevel::Observed,
        ..MemoryScope::default()
    }
}
/// Sled releases its file lock only when its last background job drops the
/// shared file handle, which can be shortly after the previous `Memory` was
/// dropped (or, after `restart`, shortly after the old process exits). Wait a
/// bounded time for that release; every other failure is returned at once.
const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(2);
fn open_store(path: &Path, wait: std::time::Duration) -> Result<SledMemoryStore, String> {
    let deadline = std::time::Instant::now() + wait;
    let mut pause = std::time::Duration::from_millis(5);
    loop {
        let mut config = EngineConfig::default();
        config.compaction.summarize_after_record_count = 2;
        match SledMemoryStore::open(SledStoreConfig::new(path).with_engine_config(config)) {
            Ok(store) => return Ok(store),
            // sled 0.34 reports contention only as this message (ErrorKind::Other).
            Err(e)
                if e.to_string().contains("could not acquire lock")
                    && std::time::Instant::now() + pause < deadline =>
            {
                std::thread::sleep(pause);
                pause = (pause * 2).min(std::time::Duration::from_millis(100));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}
impl Memory {
    pub fn open(path: &Path) -> Result<Self, String> {
        Self::open_with_learning(path, false)
    }
    pub fn open_with_learning(path: &Path, enabled: bool) -> Result<Self, String> {
        open_store(path, LOCK_WAIT).map(|store| Self {
            policy: tokio::sync::watch::channel(LearningPolicy { enabled, epoch: 0 }).0,
            store,
            mutations: tokio::sync::Mutex::new(()),
            revision: std::sync::atomic::AtomicU64::new(1),
            maintained: std::sync::atomic::AtomicU64::new(0),
            semantic_reviewed: std::sync::atomic::AtomicU64::new(0),
        })
    }
    pub async fn remember(&self, text: &str, thread: &str) -> Result<String, String> {
        let _gate = self.mutations.lock().await;
        self.remember_unlocked(text, thread, None).await
    }
    async fn remember_unlocked(
        &self,
        text: &str,
        thread: &str,
        turn: Option<&str>,
    ) -> Result<String, String> {
        if text.trim().is_empty() || text.len() > 4000 {
            return Err("Memory note must contain 1–4,000 bytes".into());
        }
        let stats = self
            .store
            .stats(StoreStatsRequest {
                tenant_id: Some("local".into()),
                namespace: Some("notdot".into()),
            })
            .await
            .map_err(|e| e.to_string())?;
        if stats.total_records >= 2000 {
            return Err(
                "Memory ledger reached 2,000 records; export or review it before adding more"
                    .into(),
            );
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis() as u64;
        let id = uuid::Uuid::new_v4().to_string();
        let mut record = MemoryRecord {
            id: id.clone(),
            scope: scope(),
            kind: MemoryRecordKind::Fact,
            content: text.into(),
            summary: None,
            source_id: Some(thread.into()),
            metadata: BTreeMap::from([(
                "origin".into(),
                "user explicitly requested retention".into(),
            )]),
            quality_state: MemoryQualityState::Active,
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
            expires_at_unix_ms: None,
            importance_score: 0.8,
            artifact: None,
            episode: None,
            historical_state: MemoryHistoricalState::default(),
            lineage: Vec::new(),
            conflict: None,
        };
        if let Some(turn) = turn {
            record.metadata.insert("source_turn".into(), turn.into());
        }
        self.store
            .upsert(UpsertRequest {
                record,
                idempotency_key: Some(id.clone()),
            })
            .await
            .map_err(|e| e.to_string())?;
        self.revision
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(id)
    }
    pub async fn recall(&self, query: &str, max: usize) -> Result<Vec<MemoryRecord>, String> {
        self.store
            .recall(RecallQuery {
                scope: scope(),
                query_text: query.chars().take(4000).collect(),
                max_items: max.min(100),
                token_budget: Some(2000),
                filters: RecallFilters::default(),
                include_explanation: true,
            })
            .await
            .map(|r| {
                r.hits
                    .into_iter()
                    .map(|h| h.record)
                    .filter(|r| {
                        matches!(
                            r.quality_state,
                            MemoryQualityState::Active | MemoryQualityState::Verified
                        )
                    })
                    .collect()
            })
            .map_err(|e| e.to_string())
    }
    pub async fn review(&self, query: &str) -> Result<Vec<MemoryRecord>, String> {
        let package = self
            .store
            .export(ExportRequest {
                tenant_id: Some("local".into()),
                namespace: Some("notdot".into()),
                include_archived: false,
            })
            .await
            .map_err(|e| e.to_string())?;
        let query = query.to_lowercase();
        let mut records: Vec<_> = package
            .records
            .into_iter()
            .map(|r| r.record)
            .filter(|r| {
                !matches!(
                    r.quality_state,
                    MemoryQualityState::Deleted | MemoryQualityState::Archived
                ) && (r.quality_state != MemoryQualityState::Suppressed || learning::provisional(r))
                    && r.content.to_lowercase().contains(&query)
            })
            .collect();
        records.sort_by_key(|r| std::cmp::Reverse(r.updated_at_unix_ms));
        records.truncate(100);
        Ok(records)
    }
    pub async fn forget(&self, id: &str) -> Result<(), String> {
        if id.len() > 128 {
            return Err("Invalid memory note ID".into());
        }
        let _gate = self.mutations.lock().await;
        self.policy
            .send_modify(|p| p.epoch = p.epoch.wrapping_add(1));
        let package = self
            .store
            .export(ExportRequest {
                tenant_id: Some("local".into()),
                namespace: Some("notdot".into()),
                include_archived: true,
            })
            .await
            .map_err(|e| e.to_string())?;
        let records: Vec<_> = package.records.into_iter().map(|r| r.record).collect();
        let source = records
            .iter()
            .find(|r| r.id == id)
            .ok_or("Memory note was not found")?;
        let mut ids = std::collections::BTreeSet::from([id.to_string()]);
        for r in &records {
            if r.content.trim().eq_ignore_ascii_case(source.content.trim())
                || source
                    .metadata
                    .get("semantic_key")
                    .is_some_and(|key| r.metadata.get("semantic_key") == Some(key))
                || r.metadata
                    .get("evidence")
                    .is_some_and(|quote| quote.trim().eq_ignore_ascii_case(source.content.trim()))
                || source
                    .metadata
                    .get("evidence")
                    .is_some_and(|quote| quote.trim().eq_ignore_ascii_case(r.content.trim()))
            {
                ids.insert(r.id.clone());
            }
        }
        loop {
            let before = ids.len();
            for r in &records {
                if r.kind == MemoryRecordKind::Summary
                    && r.lineage.iter().any(|l| ids.contains(&l.record_id))
                {
                    ids.insert(r.id.clone());
                }
            }
            if before == ids.len() {
                break;
            }
        }
        let mut ids: Vec<_> = ids.into_iter().collect();
        ids.sort_by_key(|id| {
            if records
                .iter()
                .any(|r| r.id == *id && r.kind == MemoryRecordKind::Summary)
            {
                0
            } else {
                1
            }
        });
        for record_id in ids {
            self.store
                .delete(DeleteRequest {
                    tenant_id: "local".into(),
                    namespace: "notdot".into(),
                    record_id,
                    hard_delete: true,
                    audit_reason: "User requested forgetting note and derived summaries".into(),
                })
                .await
                .map_err(|e| e.to_string())?;
        }
        self.revision
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
    /// Advances on every change to the ledger, including edits and forgetting.
    pub fn revision(&self) -> u64 {
        self.revision.load(std::sync::atomic::Ordering::Relaxed)
    }
    /// Current, reviewable facts and summaries for an encrypted backup.
    /// Provisional, archived and superseded records are not included.
    pub async fn backup_records(&self) -> Result<Vec<BackupRecord>, String> {
        let _gate = self.mutations.lock().await;
        let package = self
            .store
            .export(ExportRequest {
                tenant_id: Some("local".into()),
                namespace: Some("notdot".into()),
                include_archived: false,
            })
            .await
            .map_err(|e| e.to_string())?;
        let records: Vec<_> = package
            .records
            .into_iter()
            .map(|r| r.record)
            .filter(|r| {
                matches!(
                    r.quality_state,
                    MemoryQualityState::Active | MemoryQualityState::Verified
                ) && matches!(r.kind, MemoryRecordKind::Fact | MemoryRecordKind::Summary)
            })
            .take(MAX_RECORDS)
            .collect();
        let ids: std::collections::BTreeSet<_> = records.iter().map(|r| r.id.clone()).collect();
        Ok(records
            .into_iter()
            .map(|r| BackupRecord {
                summary: r.kind == MemoryRecordKind::Summary,
                content: r.content,
                metadata: r
                    .metadata
                    .into_iter()
                    .filter(|(k, _)| BACKUP_METADATA.contains(&k.as_str()))
                    .collect(),
                created_unix_ms: r.created_at_unix_ms,
                updated_unix_ms: r.updated_at_unix_ms,
                // Only links between included records; history stays local.
                sources: r
                    .lineage
                    .into_iter()
                    .filter(|l| ids.contains(&l.record_id))
                    .map(|l| l.record_id)
                    .collect(),
                id: r.id,
            })
            .collect())
    }
    /// IDs of current records whose content matches, for restore de-duplication.
    pub async fn current_contents(&self) -> Result<BTreeMap<String, String>, String> {
        Ok(self
            .backup_records()
            .await?
            .into_iter()
            .map(|r| (r.content.trim().to_lowercase(), r.id))
            .collect())
    }
    /// Insert validated restored records with caller-chosen IDs. Writes are
    /// idempotent per ID; the caller journals the IDs first so a failure or
    /// crash can remove exactly these records with [`Memory::remove_records`].
    pub async fn import_records(
        &self,
        records: &[BackupRecord],
        expected_revision: u64,
    ) -> Result<(), String> {
        let _gate = self.mutations.lock().await;
        if self.revision() != expected_revision {
            return Err("Memories changed after the preview".into());
        }
        self.policy
            .send_modify(|p| p.epoch = p.epoch.wrapping_add(1));
        let stats = self
            .store
            .stats(StoreStatsRequest {
                tenant_id: Some("local".into()),
                namespace: Some("notdot".into()),
            })
            .await
            .map_err(|e| e.to_string())?;
        if stats.total_records as usize + records.len() > MAX_RECORDS {
            return Err(
                "Restoring would pass the 2,000 memory limit. Nothing was restored.".into(),
            );
        }
        for r in records {
            let mut metadata: BTreeMap<String, String> = r
                .metadata
                .iter()
                .filter(|(k, _)| BACKUP_METADATA.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            metadata.insert("restored".into(), "encrypted backup".into());
            let record = MemoryRecord {
                id: r.id.clone(),
                scope: MemoryScope {
                    source: "restored-user-memory".into(),
                    ..scope()
                },
                kind: if r.summary {
                    MemoryRecordKind::Summary
                } else {
                    MemoryRecordKind::Fact
                },
                content: r.content.clone(),
                summary: None,
                source_id: None,
                metadata,
                quality_state: MemoryQualityState::Active,
                created_at_unix_ms: r.created_unix_ms,
                updated_at_unix_ms: r.updated_unix_ms,
                expires_at_unix_ms: None,
                importance_score: 0.8,
                artifact: None,
                episode: None,
                historical_state: MemoryHistoricalState::default(),
                lineage: r
                    .sources
                    .iter()
                    .map(|id| mnemara::LineageLink {
                        record_id: id.clone(),
                        relation: if r.summary {
                            mnemara::LineageRelationKind::ConsolidatedFrom
                        } else {
                            mnemara::LineageRelationKind::Supersedes
                        },
                        confidence: 1.0,
                    })
                    .collect(),
                conflict: None,
            };
            self.store
                .upsert(UpsertRequest {
                    record,
                    idempotency_key: Some(format!("restore-{}", r.id)),
                })
                .await
                .map_err(|e| e.to_string())?;
        }
        self.revision
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
    /// Remove records written by an incomplete restore. Missing IDs are fine.
    pub async fn remove_records(&self, ids: &[String]) -> Result<(), String> {
        let _gate = self.mutations.lock().await;
        self.policy
            .send_modify(|p| p.epoch = p.epoch.wrapping_add(1));
        let present: std::collections::BTreeSet<_> = self
            .store
            .export(ExportRequest {
                tenant_id: Some("local".into()),
                namespace: Some("notdot".into()),
                include_archived: true,
            })
            .await
            .map_err(|e| e.to_string())?
            .records
            .into_iter()
            .map(|r| r.record.id)
            .collect();
        for id in ids.iter().filter(|id| present.contains(*id)) {
            self.store
                .delete(DeleteRequest {
                    tenant_id: "local".into(),
                    namespace: "notdot".into(),
                    record_id: id.clone(),
                    hard_delete: true,
                    audit_reason: "Rolled back an incomplete restore".into(),
                })
                .await
                .map_err(|e| e.to_string())?;
        }
        self.revision
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
    pub async fn maintain(&self) -> Result<String, String> {
        let _gate = self.mutations.lock().await;
        self.maintain_unlocked().await
    }
    async fn maintain_unlocked(&self) -> Result<String, String> {
        let revision = self.revision.load(std::sync::atomic::Ordering::Relaxed);
        if self.maintained.load(std::sync::atomic::Ordering::Relaxed) == revision {
            return Ok("No memory changes since the previous review".into());
        }
        // Compaction can change source visibility, including on partial failure.
        let revision = self
            .revision
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        let report = self
            .store
            .compact(CompactionRequest {
                tenant_id: "local".into(),
                namespace: Some("notdot".into()),
                dry_run: false,
                reason: "Scheduled deduplication with provenance".into(),
            })
            .await
            .map_err(|e| e.to_string())?;
        let proposals = self
            .store
            .synthesize(SynthesisRequest {
                tenant_id: "local".into(),
                namespace: Some("notdot".into()),
                actor_id: Some("user".into()),
                max_source_records: 12,
                max_proposals: 8,
                dry_run: false,
                ..SynthesisRequest::default()
            })
            .await
            .map_err(|e| e.to_string())?;
        self.maintained
            .store(revision, std::sync::atomic::Ordering::Relaxed);
        serde_json::to_string_pretty(&(report, proposals)).map_err(|e| e.to_string())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn forgetting_releases_current_record_capacity_instead_of_accumulating_tombstones() {
        let _fixture = TEST_STORE.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let memory = Memory::open(dir.path()).unwrap();
        let id = memory
            .remember("Synthetic preference to forget", "fixture")
            .await
            .unwrap();
        memory
            .remember("Independent retained preference", "fixture")
            .await
            .unwrap();
        memory.forget(&id).await.unwrap();
        let stats = memory
            .store
            .stats(StoreStatsRequest {
                tenant_id: Some("local".into()),
                namespace: Some("notdot".into()),
            })
            .await
            .unwrap();
        assert_eq!(
            stats.total_records, 1,
            "Forgetting must free a current-record slot so a full ledger can accept a replacement"
        );
        memory
            .remember("Replacement preference", "fixture")
            .await
            .unwrap();
        assert_eq!(memory.review("").await.unwrap().len(), 2);
    }
    #[test]
    fn reopening_waits_only_for_a_briefly_held_store_lock() {
        let d = tempfile::tempdir().unwrap();
        let held = open_store(d.path(), LOCK_WAIT).unwrap();
        // A lock that stays held fails after the bounded wait, naming the cause.
        let started = std::time::Instant::now();
        let error = open_store(d.path(), std::time::Duration::from_millis(60))
            .expect_err("held store must not open twice");
        assert!(error.contains("could not acquire lock"), "{error}");
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        // A lock released shortly afterwards, as by sled's background jobs or an
        // exiting previous process, is waited for instead of failing startup.
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(150));
            drop(held);
        });
        open_store(d.path(), LOCK_WAIT).unwrap();
        release.join().unwrap();
        // Other failures are not retried.
        let file = d.path().join("not-a-directory");
        std::fs::write(&file, b"x").unwrap();
        let started = std::time::Instant::now();
        assert!(open_store(&file, LOCK_WAIT).is_err());
        assert!(started.elapsed() < std::time::Duration::from_millis(500));
    }
    #[tokio::test]
    async fn recall_reopen_forget() {
        let _store_fixture = TEST_STORE.lock().await;
        let d = tempfile::tempdir().unwrap();
        let id;
        {
            let m = Memory::open(d.path()).unwrap();
            id = m
                .remember("I prefer tea without sugar", "test-thread")
                .await
                .unwrap();
            assert_eq!(
                m.recall("tea", 5).await.unwrap()[0].source_id.as_deref(),
                Some("test-thread")
            );
        }
        let m = Memory::open(d.path()).unwrap();
        assert_eq!(m.recall("tea", 5).await.unwrap()[0].id, id);
        m.forget(&id).await.unwrap();
        assert!(m.recall("tea", 5).await.unwrap().is_empty());
    }
    #[tokio::test]
    async fn forgetting_retracts_derived_summaries() {
        let _store_fixture = TEST_STORE.lock().await;
        let d = tempfile::tempdir().unwrap();
        let m = Memory::open(d.path()).unwrap();
        let id = m
            .remember("My preferred tea is jasmine", "a")
            .await
            .unwrap();
        m.remember("I drink jasmine tea every morning", "a")
            .await
            .unwrap();
        m.maintain().await.unwrap();
        let records = m.review("").await.unwrap();
        assert!(records.iter().any(|r| r.kind == MemoryRecordKind::Summary));
        m.forget(&id).await.unwrap();
        let records = m.review("").await.unwrap();
        assert!(!records.iter().any(|r| r.kind == MemoryRecordKind::Summary));
        assert!(!records.iter().any(|r| r.id == id));
    }
}

/// Only explicit requests become durable notes; ordinary conversational text is not retained here.
pub fn explicit_note(text: &str) -> Option<&str> {
    let text = text.trim();
    [
        "please remember that ",
        "please remember ",
        "remember that ",
        "remember: ",
        "remember ",
    ]
    .into_iter()
    .find_map(|prefix| {
        text.get(..prefix.len())
            .filter(|head| head.eq_ignore_ascii_case(prefix))
            .and_then(|_| text.get(prefix.len()..))
            .map(str::trim)
            .filter(|note| !note.is_empty())
    })
}
#[cfg(test)]
mod note_tests {
    use super::explicit_note;
    #[test]
    fn retention_requires_explicit_request() {
        assert_eq!(
            explicit_note("Please remember that I prefer tea."),
            Some("I prefer tea.")
        );
        assert_eq!(
            explicit_note("Remember: my colour is blue"),
            Some("my colour is blue")
        );
        assert_eq!(explicit_note("I remember that holiday"), None);
        assert_eq!(explicit_note("Remember "), None);
    }
}
