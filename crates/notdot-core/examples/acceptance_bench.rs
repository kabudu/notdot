//! Bounded local-ledger measurements. Synthetic data; no runtime, voice or user state.
use notdot_core::memory::Memory;
use serde_json::json;
use std::time::Instant;
fn tag(mut index: usize) -> String {
    let mut letters = [b'a'; 6];
    for letter in letters.iter_mut().rev() {
        *letter += (index % 26) as u8;
        index /= 26;
    }
    format!(
        "synthtopic{}",
        String::from_utf8(letters.to_vec()).expect("ASCII tag")
    )
}
fn distribution(mut samples: Vec<f64>) -> serde_json::Value {
    samples.sort_by(f64::total_cmp);
    let n = samples.len();
    json!({"n":n,"min_ms":samples[0],"p50_ms":samples[n/2],"p95_ms":samples[(n*95/100).min(n-1)],"max_ms":samples[n-1]})
}
#[tokio::main]
async fn main() -> Result<(), String> {
    let count = std::env::args()
        .nth(1)
        .unwrap_or("100".into())
        .parse::<usize>()
        .map_err(|e| e.to_string())?;
    if !(10..=2000).contains(&count) {
        return Err("Use 10–2,000 synthetic records".into());
    }
    let record_bytes = std::env::args()
        .nth(2)
        .map(|s| s.parse::<usize>().map_err(|e| e.to_string()))
        .transpose()?
        .unwrap_or(0);
    if record_bytes > 4000 {
        return Err("At most 4,000 bytes per record".into());
    }
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let start = Instant::now();
    let memory = Memory::open(&dir.path().join("ledger"))?;
    let open_ms = start.elapsed().as_secs_f64() * 1000.;
    let mut insert = Vec::new();
    for i in 0..count {
        let start = Instant::now();
        let topic = tag(i);
        let mut text = format!(
            "Synthetic project {i}: I prefer focused implementation notes for {topic}, with British spelling and a short explanation of the decisions."
        );
        if record_bytes > text.len() {
            text.push_str(&" detail".repeat((record_bytes - text.len()).div_ceil(7)));
            text.truncate(record_bytes);
        }
        memory.remember(&text, "acceptance-synthetic").await?;
        insert.push(start.elapsed().as_secs_f64() * 1000.);
    }
    let mut recall = Vec::new();
    let mut hits = 0;
    for run in 0..40 {
        let query = format!("project {}", tag(run % count));
        let start = Instant::now();
        let result = memory.recall(&query, 6).await?;
        recall.push(start.elapsed().as_secs_f64() * 1000.);
        let expected = format!("Synthetic project {}:", run % count);
        if !result
            .iter()
            .any(|record| record.content.starts_with(&expected))
        {
            return Err(format!(
                "Recall missed the requested synthetic record: {expected}"
            ));
        }
        hits += result.len();
    }
    println!(
        "{}",
        json!({"workload":"synthetic-local-ledger","records":count,"record_bytes":record_bytes,"percentile_method":"nearest rank","mode":if cfg!(debug_assertions){"debug"}else{"release"},"open_ms":open_ms,"insert":distribution(insert),"recall":distribution(recall),"completed_queries":40,"returned_hits":hits,"failures":0,"excludes":"Voice, model inference, renderer and hardware latency"})
    );
    Ok(())
}
