# Task completion follow-ups, 4 November 2026

## Scope

Branch `codex/task-completion-followups`, based on master `51170d9`. Validation was performed against the feature working tree, including host-verified notification consent, owned-turn completion receipts, durable follow-up delivery and the real WASM UI. No release has been published for this feature.

## Results

- Full `scripts/ci-local.sh` passed: shipping-profile Rust tests, wake fixtures, native/UI Clippy, formatting, accessibility/content/tooling checks and 102 Python tests. Focused native/core tests and Clippy were rerun after final report-length, queue-admission and persistence-test changes.
- Final native app tests: 73 passed, seven explicitly ignored. Core library: 42 passed, two ignored. Unix bridge integration: 22 passed, three ignored. Ignored tests require separately configured live environments; they are not counted as passes.
- The bounded Python scorecard evaluator rejects missing evidence, generated-only speech, insufficient attempts, false-wake rates, unauthorised actions, failed cancellation scenarios, invalid/non-finite timings and p95 regressions. Its tests pass. No human acoustic scorecard has been asserted as passing.
- Runtime integration tests cover a completion arriving before the send acknowledgement, duplicate/foreign/stale events, tool-output exclusion, honest failure/interruption/approval/disconnection messages, opt-out, 24-receipt backpressure and capacity recovery after acknowledgement.
- Native journal tests save/reopen the report, deduplicate repeated completion events and prevent redelivery after a durable claim. Core policy tests cover Reactive mode, quiet hours, five-second spacing and dismissal.
- Authenticated `session_control_smoke` passed against the installed Codex runtime (`0.166.0-alpha.6`), using Terra Medium and synthetic chats only. Model-prepared proposals did not post before confirmation; the confirmed send produced its synthetic marker in the actual final reply and exactly one owned completion receipt. Steer and stop checks passed. These tests used the real Codex runtime but did not use microphone, STT or speakers.
- All 25 browser checks passed in the real WASM interface with synthetic IPC/audio adapters. They cover settings persistence, one report with follow-ups off, opt-in capture after report playback, spoken stop, mute deferral, Escape cancellation and no playback/capture overlap. Minimum observed echo gap: 667 ms. This is UI integration evidence, not human voice accuracy or acoustic echo evidence.

## Remaining acceptance

The new feature still needs a microphone-to-model-to-audible-task-report check and acoustic echo/cancellation checks on the candidate Mac app, plus native Windows/Linux announcement and follow-up checks. No production keys were read or changed, no QA app was installed and the existing installed app was left unchanged during these tests. The new human scorecard baselines are also pending; generated clips and mocks must not be used to fill them.

Running-task associations are intentionally in memory: keep notdot open while delegated work runs. Already saved reports survive restart, but notdot does not automatically resume a running task after restarting. A completed Codex turn is not independent verification of external work.

## OpenAI voice investigation

See the [voice assessment](../research/openai-voice.md). Both bounded V2 and V3 experimental voice probes rejected the existing subscription sign-in with `realtime conversation requires API key auth`; a successful catalog query was not treated as a working voice connection.
