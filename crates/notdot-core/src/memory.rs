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
impl Memory {
    pub fn open(path: &Path) -> Result<Self, String> {
        Self::open_with_learning(path, false)
    }
    pub fn open_with_learning(path: &Path, enabled: bool) -> Result<Self, String> {
        let mut config = EngineConfig::default();
        config.compaction.summarize_after_record_count = 2;
        SledMemoryStore::open(SledStoreConfig::new(path).with_engine_config(config))
            .map(|store| Self {
                policy: tokio::sync::watch::channel(LearningPolicy { enabled, epoch: 0 }).0,
                store,
                mutations: tokio::sync::Mutex::new(()),
                revision: std::sync::atomic::AtomicU64::new(1),
                maintained: std::sync::atomic::AtomicU64::new(0),
                semantic_reviewed: std::sync::atomic::AtomicU64::new(0),
            })
            .map_err(|e| e.to_string())
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
            .map(|r| r.hits.into_iter().map(|h| h.record).collect())
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
                    MemoryQualityState::Deleted
                        | MemoryQualityState::Suppressed
                        | MemoryQualityState::Archived
                ) && r.content.to_lowercase().contains(&query)
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
                    hard_delete: false,
                    audit_reason: "User requested forgetting note and derived summaries".into(),
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
