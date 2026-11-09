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

The compiled production WASM passed twelve silent UI integration checks: missing-installation guidance, cancelled sign-in recovery, voice-settings navigation, saving without completing setup, returning from a simulated spoken exchange to explicit audible confirmation, and finishing at the orb. Authentication, STT, TTS and audio were simulated; there were no microphone or account requests in that browser test.

Reproduce the silent preview with `trunk build --release`, `python3 scripts/prepare_setup_preview.py`, then serve `.local/setup-preview` on loopback and inspect `#harness-results`. It must say `passed`.

Native tests validate signed-out and malformed account states, HTTPS handoff restrictions and expiry cancelling a managed login before releasing its actual fixture runtime. A running process alone is not treated as proof of sign-in. The first-run test requires a native voice request, spoken playback state and an explicit user confirmation; a saved form or voice preview alone does not finish setup.

Installation is guided through OpenAI's official installation page. notdot does not silently install Codex or promise to bypass installer, microphone, browser or OS consent. Cancelled setup can be resumed using saved settings; pending login callbacks are not persisted across a restart.

## Proactive engagement and privacy

Tests bind explanation, snooze and dismissal to one exact update spoken within two minutes. Missing, stale, expired or already-dismissed references produce a request to select an item, not a guessed action. Snoozing retains quiet hours and frequency limits. Dismissal retains the source tombstone so discovery cannot resurrect the same saved item before it expires. Requested task-result permissions remain separate from unsolicited check-ins.

Existing native gates still require an idle, visible, unmuted app, current profile access and permitted announcements. No new external-action authority is granted. The troubleshooting export uses an explicit field allowlist and is never uploaded automatically.

## Native desktop acceptance

On the ARM64 Linux VM (XFCE, WebKitGTK 2.52.6), a locally built production app opened the new setup screen in a fresh, separate data directory. It recognised the existing Codex sign-in, opened Voice settings, accessed both saved provider keys without re-entry, loaded the voice catalogue and saved the chosen voice while leaving setup incomplete. The existing profile was untouched.

That check exposed competing native resize requests when saving Settings and reopening setup: the window could collapse and hide the remaining buttons. Native size changes now run in request order. The regression check exercises the actual bridge JavaScript with a delayed first resize and verifies that setup remains expanded and interactive. The rebuilt native Linux app passed the same Settings-to-setup transition with all action buttons visible.

The Linux first-run trial also completed a synthetic speaker-to-microphone request through Deepgram, Terra Medium and ElevenLabs. The saved test chat contains “Please calculate five plus seven an answer in one short sentence” and the answer “Five plus seven equals twelve.” Orb telemetry records listening, thinking and speaking, followed by the audible-confirmation screen. Setup remained incomplete pending that explicit confirmation. This establishes the automated pipeline, not a human judgement of audio quality.

The Windows laptop is connected and its installed Codex reports an existing ChatGPT sign-in. The installer from successful CI run 42621358940 was downloaded, extracted and transferred with a pinned SHA-256 check. Avast blocked the unsigned executable for Threat Labs analysis before it could launch. Protection was left enabled; the original Windows profile was restored and verified. Windows microphone-to-reply acceptance remains blocked by that scan.

The Developer ID-signed native Mac build detected the existing Codex account, opened setup and Voice settings, loaded ElevenLabs voices from the saved key and returned from Settings at the correct expanded size. No provider key was exported or re-entered. Both Deepgram and ElevenLabs transcription then completed synthetic speaker-to-microphone turns through Terra Medium and ElevenLabs playback. The saved test chat contains the correct arithmetic answer. The orb reported thinking and speaking and setup returned to explicit completion confirmation. The completion button saved the flag, and restarting returned to the orb without reopening setup. Human judgement of audible quality is not claimed by this automated check.

Earlier failed Mac attempts were not reliably synchronised with the eight-second listening window. A temporary private recording captured only 2.08 seconds of the 4.18-second source clip; direct local VAD testing recognised speech in that recording. The corrected test waits for a fresh native listening report before playing the phrase. A controlled original-level test passed with Deepgram; ElevenLabs returned an empty transcript on one attempt at that level, then passed an original-level retest as well as the louder fixture. This is not evidence of equal recognition accuracy across providers. The temporary recording instrumentation was removed from the source and final app before retesting. No microphone audio was committed. Empty ElevenLabs transcripts were also being classified as provider faults, allowing a second billable batch attempt. Empty results now use the existing NoSpeech outcome; missing, malformed and oversized responses remain provider errors. Tests prove that NoSpeech cannot request batch fallback or send an empty request to the model. A long-turn regression also checks that an empty final segment preserves previously recognised words; only an entirely empty utterance gets NoSpeech.

The investigation also reproduced two setup recovery defects. An empty capture left the user at the orb until the full setup timeout; it now immediately returns to setup with a microphone/retry message. Escape could open recovery and then dismiss that new dialog through its default action; cancellation consumes Escape only while opening recovery, preserving the normal Escape-to-close action of an already-open dialog. Both native behaviours passed. Native Mac silence, cancellation and successful retry passed after rebuilding. A silence trial after successful playback correctly removed the completion option, so an earlier successful trial cannot finish a later empty one. The original Mac profile was restored after testing. The rebuilt Linux app also passed native silence and Escape recovery, a synchronised voice retry and explicit setup completion. After the final empty-segment change (1a4b070), both Mac and Linux repeated the Deepgram → Terra Medium → ElevenLabs turn and returned to idle. The Mac run also repeated explicit setup completion. The ARM64 Linux build passed all 21 voice tests. These checks used synthetic audio, not a real-person listening assessment.

## Final native onboarding checks

The final production Mac binary at 1a4b070 was launched with a separate, initially signed-out Codex home and a disposable notdot profile. The normal Codex credentials and notdot profile were not altered. File-based credential storage was selected only for the isolated test runtime.

- Signed-out detection displayed Sign in with ChatGPT.
- The actual runtime opened OpenAI’s browser account-selection page. Cancelling in notdot returned immediately to sign-in guidance; retry started a new browser flow.
- Selecting the existing browser account and continuing the official Codex sign-in completed the real localhost callback. notdot automatically reported Codex connected and exposed voice setup and the conversation test.
- Sign-in alone left onboarding incomplete. Continue later returned to the orb; restarting reused the new sign-in and reopened the unfinished setup without another browser login.
- Selecting a deliberately nonexistent runtime displayed Get Codex and Check again. Get Codex opened the official quickstart with desktop downloads and CLI guidance; retry remained actionable while the runtime was absent.

The temporary app was stopped, its registration removed and the original notdot profile restored. The temporary runtime authentication file was removed without reading or exporting its contents. This verifies a fresh Codex credential store using an existing browser account, not account creation, password entry or every operating system’s consent UI.

## Remaining acceptance boundaries

- Final code CI run [42632438616](https://github.com/kabudu/notdot/actions/runs/42632438616), at 1a4b070, passed every job: Apple Silicon and Intel macOS, Linux native, Windows native and installer packaging, release tooling and content checks. Superseded runs were cancelled to avoid redundant builds.
- Fresh Codex sign-in, browser return and restart recovery passed natively on Mac as described above. Account creation and user-owned OS consent remain outside unattended acceptance. Windows native CI and installer packaging passed; laptop voice execution remains blocked by Avast analysis. The maintainer explicitly accepted this hardware test gap for v0.7.0 on 9 November 2026; it does not block merge. Protection was not disabled.
- Installed-update, battery, long-duration RAM and physical Bluetooth/device-switch measurements are separate hardware acceptance checks; this report does not mark them passed.
