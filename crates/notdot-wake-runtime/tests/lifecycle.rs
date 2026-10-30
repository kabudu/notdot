use notdot_wake_runtime::{
    client::{Client, Failure, all_helpers_reaped},
    engine, protocol,
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
// Each scenario follows the app's single-helper lifecycle. Competing model
// loads from unrelated tests must not consume its real startup deadline.
static HELPER_SCENARIO: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn helper() -> PathBuf {
    std::env::var_os("NOTDOT_HELPER_TEST_EXE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_notdot-wake-helper")))
}
// Cancellation tests exercise actual Client pipe I/O and child kill/reap without
// spending its production startup deadline on a cold debug model hash/load.
// Recognition and invalid-model acceptance still use the real helper below.
fn protocol_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_notdot-wake-protocol-fixture"))
}
fn fixture_root() -> PathBuf {
    std::env::var_os("NOTDOT_TEST_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../src-tauri"))
}
fn model() -> PathBuf {
    fixture_root()
        .join("resources/wake/ggml-tiny.en-q5_1.bin")
        .canonicalize()
        .unwrap()
}
#[tokio::test]
async fn cancellation_and_timeout_kill_and_reap_the_helper() {
    let _scenario = HELPER_SCENARIO.lock().await;
    let mut cancelled = Client::start(&protocol_fixture(), &model(), "Hey Ava")
        .await
        .unwrap();
    assert!(cancelled.pid().is_some());
    assert_eq!(
        cancelled
            .recognize(
                &[0.; 320],
                Arc::new(AtomicU64::new(1)),
                0,
                Duration::from_secs(3)
            )
            .await,
        Err(Failure::Cancelled)
    );
    assert!(cancelled.pid().is_none());
    let mut expired = Client::start(&protocol_fixture(), &model(), "Hey Ava")
        .await
        .unwrap();
    assert_eq!(
        expired
            .recognize(&[0.; 320], Arc::new(AtomicU64::new(0)), 0, Duration::ZERO)
            .await,
        Err(Failure::Unavailable)
    );
    assert!(expired.pid().is_none());
    assert!(all_helpers_reaped());
}
#[tokio::test]
async fn cancellation_after_request_begins_reaps_before_the_install_hook() {
    let _scenario = HELPER_SCENARIO.lock().await;
    let mut client = Client::start(&protocol_fixture(), &model(), "Hey Ava")
        .await
        .unwrap();
    let epoch = Arc::new(AtomicU64::new(0));
    let audio = vec![0.01; protocol::MAX_SAMPLES];
    {
        let request = client.recognize(&audio, epoch.clone(), 0, Duration::from_secs(3));
        tokio::pin!(request);
        // Poll actual pipe I/O first, rather than cancelling before recognize starts.
        std::future::poll_fn(|cx| {
            assert!(request.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        epoch.store(1, Ordering::Relaxed);
        assert_eq!(request.await, Err(Failure::Cancelled));
    }
    // Installer replacement cannot run until the old helper has been reaped.
    let failed_install = || -> Result<(), ()> {
        assert!(client.pid().is_none());
        assert!(all_helpers_reaped());
        Err(())
    };
    assert!(failed_install().is_err());
    let mut restarted = Client::start(&protocol_fixture(), &model(), "Hey Ava")
        .await
        .unwrap();
    assert!(restarted.pid().is_some());
    assert!(restarted.shutdown().await);
    assert!(restarted.pid().is_none());
    assert!(all_helpers_reaped());
}
#[tokio::test]
async fn invalid_model_or_phrase_never_starts_a_ready_worker() {
    let _scenario = HELPER_SCENARIO.lock().await;
    let directory = tempfile::tempdir().unwrap();
    let invalid = directory.path().join("bad-model.bin");
    std::fs::write(&invalid, b"synthetic invalid model").unwrap();
    assert!(matches!(
        Client::start(&helper(), &invalid, "Hey Ava").await,
        Err(Failure::Unavailable)
    ));
    assert!(matches!(
        Client::start(&helper(), &model(), &"a".repeat(85)).await,
        Err(Failure::Unavailable)
    ));
    assert!(protocol::parse_response(&protocol::response(4, Ok(true)), 5).is_err());
}
#[tokio::test]
#[ignore = "full bundled model decode; run on the chosen native architecture"]
async fn worker_reuses_one_model_process_and_matches_the_shared_engine() {
    let _scenario = HELPER_SCENARIO.lock().await;
    let mut client = Client::start(&helper(), &model(), "Hey Ava").await.unwrap();
    let pid = client.pid();
    for (name, expected) in [
        ("hey-ava.wav", true),
        ("hey-ava-uk.wav", true),
        ("hey-ava-us.wav", true),
        ("ava.wav", false),
        ("negative.wav", false),
    ] {
        let mut wav =
            hound::WavReader::open(fixture_root().join("tests/fixtures").join(name)).unwrap();
        let rate = wav.spec().sample_rate;
        let audio: Vec<_> = wav
            .samples::<i16>()
            .map(|v| f32::from(v.unwrap()) / 32768.)
            .collect();
        let mut audio = engine::normalize(&audio, rate).unwrap();
        audio.resize(audio.len() + 8000, 0.);
        assert_eq!(
            client
                .recognize(
                    &audio,
                    Arc::new(AtomicU64::new(0)),
                    0,
                    Duration::from_secs(3)
                )
                .await
                .unwrap(),
            expected
        );
        assert_eq!(client.pid(), pid);
    }
    client.shutdown().await;
    assert!(client.pid().is_none());
}
