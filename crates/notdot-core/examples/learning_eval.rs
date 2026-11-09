//! Explicit account-backed evaluation. Synthetic input only, no voice keys or tools.
use notdot_core::{
    bridge::Bridge,
    memory::{Interaction, LearningBatch, Memory, schema},
};
use serde_json::json;
use std::{path::PathBuf, time::Instant};
#[tokio::main]
async fn main() -> Result<(), String> {
    let runtime = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("Pass a Codex executable path")?,
    );
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let memory = Memory::open_with_learning(&dir.path().join("evaluation"), true)?;
    let bridge = Bridge::connect(&runtime).await?;
    let cases = [
        (
            "communication",
            "I prefer short answers with British spelling.",
            true,
        ),
        (
            "qualified_preference",
            "I do not drink coffee after lunch, except on holiday.",
            true,
        ),
        (
            "project",
            "I am building a birdhouse for my garden this month.",
            true,
        ),
        (
            "temporary_request",
            "For today only, please answer my questions in French.",
            false,
        ),
        (
            "hypothetical",
            "Imagine I moved to Rome. What would the weather be like?",
            false,
        ),
        (
            "quoted_fiction",
            "For example, a fictional character might say: I am allergic to apples.",
            false,
        ),
        ("acknowledgement", "Thanks!", false),
        (
            "secret",
            "Do not remember this: my API key is sk-synthetic-placeholder.",
            false,
        ),
        ("short_name", "My name is Jo.", true),
        (
            "correction",
            "My preference has changed: I now want detailed answers, still with British spelling.",
            true,
        ),
    ];
    let mut results = Vec::new();
    let mut failures = 0;
    for (index, (id, text, expected)) in cases.into_iter().enumerate() {
        let start = Instant::now();
        let plan = memory
            .plan(
                memory.learning_ticket().unwrap(),
                Some(Interaction {
                    thread: "synthetic-evaluation".into(),
                    turn: format!("turn-{index}"),
                    user: text.into(),
                }),
            )
            .await?;
        let mut retained = 0;
        let mut called = false;
        if let Some(plan) = plan {
            called = true;
            let response = bridge
                .analyze(
                    dir.path(),
                    &plan.prompt()?,
                    schema(),
                    std::future::pending(),
                )
                .await?;
            let mut batch: LearningBatch =
                serde_json::from_str(&response).map_err(|e| e.to_string())?;
            batch.ground_notes();
            batch.discard_invalid_digest(&plan);
            batch.validate_for(&plan)?;
            retained = batch.notes.len();
            if expected && retained > 0 {
                memory.apply_confirmed_learning(&plan, batch).await?;
            }
        }
        let pass = (retained > 0) == expected;
        failures += usize::from(!pass);
        results.push(json!({"case":id,"passed":pass,"proposed_notes":retained,"model_called":called,"milliseconds":start.elapsed().as_millis()}));
    }
    let recall = memory.recall("name Jo", 6).await?;
    let recalled = recall.iter().any(|r| r.content.contains("Jo"));
    failures += usize::from(!recalled);
    let active = memory.review("").await?;
    let correction_kept = active
        .iter()
        .any(|r| r.content.contains("detailed answers"))
        && active.iter().all(|r| !r.content.contains("short answers"));
    let qualification_kept = active
        .iter()
        .any(|r| r.content.contains("not drink coffee") && r.content.contains("except on holiday"));
    failures += usize::from(!correction_kept) + usize::from(!qualification_kept);
    let statements: Vec<_> = active
        .iter()
        .map(|r| json!({"key":r.metadata.get("semantic_key"),"content":r.content}))
        .collect();
    bridge.shutdown().await;
    println!(
        "{}",
        json!({"method":"Real Codex structured extraction; synthetic source; direct test-only confirmation, not voice-consent E2E","model":"GPT-6.1 Terra","effort":"medium","cases":results,"name_recalled":recalled,"correction_kept":correction_kept,"qualification_kept":qualification_kept,"active_synthetic_statements":statements,"failures":failures})
    );
    if failures > 0 {
        return Err(format!("{failures} evaluation cases failed"));
    }
    Ok(())
}
