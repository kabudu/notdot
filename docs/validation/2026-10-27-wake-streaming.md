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

Passed on the macOS QA bundle built from `859727f`, after local key approval. Tests used known spoken fixtures played through the laptop speaker into its actual microphone, the native capture path, live ElevenLabs and the actual Codex runtime; this was not a direct audio-file upload or mocked model. No demo recorder was running.

- Wake → request → streaming transcription → Terra Medium → metered spoken reply completed. The saved reply was “Seven plus five is twelve.” The persisted turn context confirms `gpt-6.1-terra`, `medium`; the settings match.
- Wake activation appeared 1.461 seconds after fixture playback began. The orb's first Listening transition followed native activation by **15 ms**, with the event counter incremented. Thinking and metered Speaking transitions followed; peak speech RMS was 0.429.
- The service accepted 53 audio chunks, sent five nonempty partial events and one committed segment. First audio upload followed activation by 185 ms. First partial arrived **3.014 seconds before capture ended**, proving transcription overlapped live recording. No batch fallback or provider error occurred.
- Capture lasted 5.314 seconds. Committed transcription completed about **220 ms after capture ended**. First Speaking transition was **3.142 seconds after capture ended**; wake listening resumed 13.070 seconds after fixture start. These are one-run observations, including provider/model variability, not a universal latency guarantee.
- Native one-click mode also passed: one click, staged spoken question, automatic silence submission, live streaming (107 chunks, five partials, one committed segment), correct spoken answer. Staging delay between the UI click and fixture playback makes its total recording duration unsuitable as a responsiveness benchmark. Wake mode was restored through Settings afterward.
- Escape cancelled active capture, stopped a spoken compiler response and aborted a separate still-active long model turn. The real runtime recorded `turn_aborted` at `2026-10-27T17:10:45.369Z`, following `task_started` at `17:10:41.754Z`. The app returned to idle/wake listening without an error.
- A further capture was cancelled while the staged request was still speaking into the active stream. It returned to wake listening without submitting a later model turn or speaking a late reply.

The user’s own voice has not yet been retested on this particular build; the previous build had passed their wake/reply and no-pop check. This acoustic test does not establish all-name/accent/noise reliability or subjective audio quality. Fresh promotional recording remains separate and unpublished.
