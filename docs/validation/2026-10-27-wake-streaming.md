# Wake feedback and streaming transcription — 27 October 2026

## Implementation

The local fallback checks a complete phrase after 200 ms of quiet (previously 300 ms). Exact phrase matching and local processing are retained. A speculative two-pass recognizer was tested and removed because it gave no measurable improvement. Busy recognition no longer discards the next buffered utterance. Native activation events update the orb without waiting for its 200 ms status poll; polling remains a fallback.

Activated microphone audio streams through the pluggable native voice interface to ElevenLabs Scribe v2 Realtime. No wake-listening audio enters that stream. Only committed segments reach the model; partial text never becomes a request. Native manual recording uses the same stream and speech endpoint. Batch-only providers retain their existing recording path. Terra / Medium is unchanged.

Queues, transcript sizes, chunks, handshakes and commit waits are bounded. Missing packets—including the final packet—reject the stream and permit a complete-recording retry. Cancellation tombstones cover requests cancelled before startup and abort active tasks; cancelled captures cannot fall back to another upload. Authentication/quota errors are not retried. Keys remain in sensitive native headers and errors never include raw socket requests or provider bodies.

## Automated evidence

- `scripts/ci-local.sh`: passed. Native app 34 tests, core 23, memory integration 7, voice 14; default/debug WASM Clippy; gesture/visual/speech queue suites; Python 10. Ignored platform/private tests remain explicitly separate.
- Native microphone pipeline fixture checks activation before packets, packet ordering/count, WAV equivalence and exclusion of background wake audio.
- Mock WebSocket checks partial rewrites versus stable commits and short-utterance padding. Session tests reject missing middle/final audio, preserve long-turn segment order and prevent retries after cancellation.
- `cargo audit --json`: zero known vulnerabilities; existing maintenance/GLib notices remain tracked in the security review. Staged-patch `gitleaks stdin --redact`: no leaks.
- macOS QA app bundle built successfully, default voice-only frontend.

## Local replay measurements

Apple M4, debug tests, real-time 20 ms packet replay of the same private acoustic microphone fixture. Two interleaved 300/200 ms comparisons: 2666 → 2562 ms and 2665 → 2570 ms from fixture start to match. This supports about 100 ms less wake delay on this fixture, not a universal recognition or full-turn speedup. A private human fixture also matched (5861 → 5764 ms, one pair). “Hey” alone, “Hey avalanche” and “Hey avocado” were rejected at the shorter pause. These small fixture sets do not establish all-name/accent/noise reliability.

## Live acceptance

Pending local saved-key approval for the rebuilt QA app. Full microphone → live streaming transcription → Terra Medium → spoken reply, native event feedback, and cancellation must be verified before claiming full acceptance. No demo recording is running or published.
