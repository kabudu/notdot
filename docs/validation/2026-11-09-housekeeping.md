# v0.7.0 housekeeping validation

This report describes implementation checks on the feature branch in PR #32. It does not certify a released v0.7.0 installer or replace native first-run account and audio acceptance.

## Memory quality

The explicit `learning_eval` example uses the real Codex runtime with GPT-6.1 Terra Medium, a temporary memory store, synthetic statements and tools disabled. It does not use voice keys or the user's memories. Its direct test-only confirmation exercises retention rather than spoken consent.

The final ten-case run passed: communication preferences, qualified preferences, an ongoing project, temporary requests, hypothetical and fictional statements, acknowledgement skipping, opt-out/secret exclusion, a short name and a correction. Name recall, the correction and the original qualification also passed. [Structured results and source hash](2026-11-09-learning-evaluation.json).

Earlier runs exposed an ongoing-project omission and a mixed-source correction bug. Instructions now distinguish ongoing projects from temporary response requests. Confirmed corrections retire sibling records containing the same outdated source quote, including derived summaries. Reaffirmed details can be retained as separate, current evidence clauses. Host tests also reject quotations that omit surrounding negation or qualifications.

Existing tests cover disabled learning, independent corroboration, provenance, editing, forgetting derived summaries, stale proposals, cancellation and separate profiles. These are bounded evaluations, not a claim that all future model extractions are correct. Spoken read-back remains required for new facts from speech.

## Local recall measurements

macOS 27.0.1, Apple Silicon, release build. Three fresh-process runs at each size, 40 independently checked recalls per run. The same synthetic workload and expected six-hit results were used before and after the changes. All expected results were retained.

| Records | Before median p50 | After median p50 | Before median p95 | After median p95 |
| --- | --- | --- | --- | --- |
| 100 | 0.537 ms | 0.544 ms | 0.798 ms | 0.860 ms |
| 1,000 | 2.535 ms | 2.438 ms | 2.774 ms | 2.757 ms |

These figures do not establish a general speed-up and exclude voice, inference and GPU work. Efficiency changes instead eliminate analysis for exact simple acknowledgements, cap serialized learning prompts at 32 KiB without truncating user evidence, close idle analysis processes after two minutes, avoid the usual full task-journal copy and cap drawing behind Settings at 10 rather than 30 frames per second. No battery-life or whole-app RAM reduction is claimed without hardware measurement.

## Guided setup

The compiled production WASM passed six silent UI integration checks: missing-installation guidance, cancelled sign-in recovery, voice-settings navigation, saving without completing setup, returning from a simulated spoken exchange to explicit audible confirmation, and finishing at the orb. Authentication, STT, TTS and audio were simulated; there were no microphone or account requests in that browser test.

Reproduce the silent preview with `trunk build --release`, `python3 scripts/prepare_setup_preview.py`, then serve `.local/setup-preview` on loopback and inspect `#harness-results`. It must say `passed`.

Native tests validate signed-out and malformed account states, HTTPS handoff restrictions and expiry cancelling a managed login before releasing its actual fixture runtime. A running process alone is not treated as proof of sign-in. The first-run test requires a native voice request, spoken playback state and an explicit user confirmation; a saved form or voice preview alone does not finish setup.

Installation is guided through OpenAI's official installation page. notdot does not silently install Codex or promise to bypass installer, microphone, browser or OS consent. Cancelled setup can be resumed using saved settings; pending login callbacks are not persisted across a restart.

## Proactive engagement and privacy

Tests bind explanation, snooze and dismissal to one exact update spoken within two minutes. Missing, stale, expired or already-dismissed references produce a request to select an item, not a guessed action. Snoozing retains quiet hours and frequency limits. Dismissal retains the source tombstone so discovery cannot resurrect the same saved item before it expires. Requested task-result permissions remain separate from unsolicited check-ins.

Existing native gates still require an idle, visible, unmuted app, current profile access and permitted announcements. No new external-action authority is granted. The troubleshooting export uses an explicit field allowlist and is never uploaded automatically.

## Remaining acceptance boundaries

- Three-platform CI must pass for the final PR revision, including native tests and packaging checks.
- Clean-account browser consent and real native first-run microphone-to-spoken-reply tests have not been established by the silent preview or extraction evaluation.
- Installed-update, battery, long-duration RAM and physical Bluetooth/device-switch measurements are separate hardware acceptance checks; this report does not mark them passed.
