# Task completion follow-ups, 4 November 2026

## Scope

Branch `codex/task-completion-followups`, based on master `51170d9`. Validation was performed against the feature working tree, including host-verified notification consent, owned-turn completion receipts, durable follow-up delivery and the real WASM UI. No release has been published for this feature. The native acoustic candidate was built from `4b3a684`; subsequent review changes suppress expected cancellation errors without changing task routing or audio capture.

## Results

- Full `scripts/ci-local.sh` passed: shipping-profile Rust tests, wake fixtures, native/UI Clippy, formatting, accessibility/content/tooling checks and 102 Python tests. Focused native/core tests and Clippy were rerun after final report-length, queue-admission and persistence-test changes.
- Final native app tests: 73 passed, seven explicitly ignored. Core library: 42 passed, two ignored. Unix bridge integration: 22 passed, three ignored. Ignored tests require separately configured live environments; they are not counted as passes.
- The bounded Python scorecard evaluator rejects missing evidence, generated-only speech, insufficient attempts, false-wake rates, unauthorised actions, failed cancellation scenarios, invalid/non-finite timings and p95 regressions. Its tests pass. No human acoustic scorecard has been asserted as passing.
- Runtime integration tests cover a completion arriving before the send acknowledgement, duplicate/foreign/stale events, tool-output exclusion, honest failure/interruption/approval/disconnection messages, opt-out, 24-receipt backpressure and capacity recovery after acknowledgement.
- Native journal tests save/reopen the report, deduplicate repeated completion events and prevent redelivery after a durable claim. Core policy tests cover Reactive mode, quiet hours, five-second spacing and dismissal.
- Authenticated `session_control_smoke` passed against the installed Codex runtime (`0.166.0-alpha.6`), using Terra Medium and synthetic chats only. Model-prepared proposals did not post before confirmation; the confirmed send produced its synthetic marker in the actual final reply and exactly one owned completion receipt. Steer and stop checks passed. These tests used the real Codex runtime but did not use microphone, STT or speakers.
- All 26 final browser checks passed in the real WASM interface with synthetic IPC/audio adapters. They cover settings persistence, one report with follow-ups off, opt-in capture after report playback, spoken stop, mute deferral, Escape cancellation expected cancellation without error messages, and no playback/capture overlap. Minimum observed echo gap: 675 ms (rounded). This is UI integration evidence, not human voice accuracy or acoustic echo evidence.

## Remaining acceptance

Native Windows/Linux announcement and follow-up checks, human listening assessments and the new human scorecard baselines remain pending. No human scorecard has been marked accepted. The installed production app and its settings were unchanged; after testing it was reopened and visibly listening for Hey Ava. The original QA profile was restored, the temporary app bundle removed and only the two synthetic native-test chats archived.

## Mac acoustic check

A temporary Developer ID-signed ARM64 Mac candidate used a disposable QA profile, Terra Medium and non-interactive saved ElevenLabs access. The isolated Deepgram key was unavailable, so this run used ElevenLabs recognition and replies; task reports used macOS system speech. No key was exposed or changed and no new Keychain approval was requested.

- A generated phrase played through the Mac speaker was captured by the real microphone and transcribed. The first attempt revealed that “when it’s done” can become “when it is done”; explicit expanded suffixes were added and regression-tested. No target message was sent before confirmation.
- The corrected run exercised spoken request, actual model-prepared proposal, generated spoken “yes, send it” approval, target response and native task announcement. The journal recorded the actual target reply, delivered once.
- Follow-up listening appeared after report playback, timed out after five seconds and returned to idle. Quiet observation produced no duplicate report or extra model request from the assistant’s own output.
- Further synthetic tasks exercised longer reports. The first cancellation observation missed the speech interval and is not counted as a speech-cancellation pass. A repeated bounded check signalled at report startup; Escape returned the orb to idle, the system speech process ended and no follow-up capture reopened.

This is generated speaker-to-microphone functional evidence, not human recognition accuracy, perceptual audio quality or a statistically established latency score.

Running-task associations are intentionally in memory: keep notdot open while delegated work runs. Already saved reports survive restart, but notdot does not automatically resume a running task after restarting. A completed Codex turn is not independent verification of external work.

## OpenAI voice investigation

See the [voice assessment](../research/openai-voice.md). Both bounded V2 and V3 experimental voice probes rejected the existing subscription sign-in with `realtime conversation requires API key auth`; a successful catalog query was not treated as a working voice connection.
