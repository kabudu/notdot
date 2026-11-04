# Local Codex chat access, session controls and wake recovery

## Scope

Changes on `codex/chat-access-wake-reliability`, following the 0.2.0 release. Testing used the Mac host, its authenticated local Codex runtime, Terra Medium, the bundled wake models and synthetic fixtures. The installed 0.2.0 app and its saved voice keys were initially untouched; the follow-up installed-app checks below used a locally built, Developer ID-signed universal replacement at the user’s request. No new QA app was installed.

## Chat access

- The locally built release-mode executable's helper entry point passed live initialization, tool discovery, chat lookup/read and rejection of an attempted write method, without starting another orb or accessing voice keys.
- Live MCP protocol checks initialized the bundled service, listed its initial read-only tools, found ten local chats and successfully read a page of messages. Private message content was not printed or committed.
- A separate model test created a synthetic named source chat containing a random marker. An ephemeral Terra Medium chat received the lookup service and a prompt that contained the title but neither the marker nor the source chat ID. It invoked both `find_chats` and `read_chat`, then returned the correct marker. The synthetic source chat was archived afterwards; the ephemeral chat was unloaded.
- Fake-runtime integration tests check filtering of reasoning, images, tool output and filesystem metadata; invalid IDs, oversized arguments, unknown tools, UTF-8 truncation, pagination and both new/resumed companion registration. These are protocol tests, not voice tests.
- The read-only service uses `thread/list` and `thread/turns/list` for retrieval and `thread/read` for action metadata. Its third tool prepares a proposal only. The host, not this service, executes approved controls. The source-chat archive above is test cleanup performed by the test driver, not a service capability.

## Session controls

The Settings modes are Read only (the default), Confirm before sending and Send when asked. Unit/protocol checks cover scoped one-time consent, expiry, cancellation before/during resume, newer-request invalidation, exact instruction preservation, ambiguous destinations, wrong tool/source-turn events, unknown modes, foreign writers and preservation of the target permission/model/cwd settings.

An authenticated synthetic end-to-end test exercised the real built helper and Codex runtime with Terra Medium. The model found the target and prepared its message, the target history remained unchanged before confirmation, the confirmed send received the correct marker reply, steering succeeded and stopping produced an interrupted target turn. Target and controller test chats were archived afterwards. No speech keys, microphone or speaker were used in this test.

A separate direct runtime diagnostic reproduced an early-stop race without steering: the first interrupt returned “no active turn”, and the same turn’s stop was accepted about 103 ms after start. Control cancellation now retries only that transient response, with a bounded backoff, the same owned turn ID and a still-valid user grant. Sends are never retried. Completion/release tracking waits for `thread/closed`; an unsubscribe acknowledgement alone is not proof of writer release.

The real WASM UI passed twenty silent integration cases with synthetic IPC/STT/TTS adapters. Added checks cover spoken session confirmation, direct actions without a confirmation capture, Escape cancelling consent, and persistence of the chosen Settings mode. Existing continuity, echo-gap, mute, hide and silence checks also passed. The final 450-pixel panel preview was also inspected for spacing, text wrapping and arrow-key selection; its heading reflects the chosen mode, and control limits are collapsed by default. These adapters never request a physical microphone or contact a voice provider. This is UI integration evidence, not a new acoustic test of the changed app.

The earlier read-only/wake revision passed Mac, Windows and Linux CI (run 41803286376). Session-control changes require a new cross-platform run; the PR records that result. A physical microphone-to-reply check of the final changed build remains a release validation item.

## Wake recovery

A regression test reproduced a concrete failure: one backup recognizer error caused `Worker::frame` to enter a permanent error state before the independent primary detector could process another frame. The original code failed the regression assertion. The fix keeps the primary detector running, reports the backup warning and permits another bounded backup decode on subsequent speech. Successful recognition clears the warning.

The expanded wake suite passed 31 tests, with five explicit diagnostic/environment tests ignored. Added cases include a previous backup failure, three positive wake recordings and two negative recordings at 2%, 5% and 20% amplitude, plus the same five recordings with low-frequency rumble and input bias. Existing extended-idle, 48 kHz, cancellation, microphone queue and speech-boundary tests also passed.

These tests do not establish the cause of the single missed acoustic activation in the installed 0.2.0 test. They establish the separately reproduced failure and its recovery. Nor do they establish all-name, all-accent, room-distance or cross-platform microphone accuracy. The real installed-app voice exchange recorded earlier remains a 0.2.0 result, not proof of this changed build.

## Checks

`scripts/ci-local.sh` passed: release-mode workspace tests, native and WASM Clippy (including the debug-only UI feature), format checks, gesture/visual/speech-queue checks and 99 Python checks. Live chat checks used the authenticated runtime without API-key exports or requests for saved voice keys. See the PR checks for Windows and Linux build results.

## Installed-app follow-up

The source build replaced `/Applications/notdot.app`, using the production bundle identifier and existing Developer ID requirement. The previous official bundle is retained as an ignored rollback ZIP, not a second installed app. Both Intel and Apple Silicon slices and the signature were verified. These local builds are signed but not notarised; public release notarisation remains in GitHub Actions. Settings and keys were preserved, with Read only added as the default setting. No new saved-key prompt was needed.

Speaker-to-microphone checks exercised Deepgram, Terra Medium, ElevenLabs playback and the real orb. A spoken lookup recovered the synthetic chat’s amber marker; the follow-up window accepted “That’s all”. A separate confirmed send left the synthetic target unchanged before “Yes, send it”, then created exactly one target turn and received the expected reply. These are generated-speaker acoustic tests, not a new human-listening assessment.

Further tests exposed two transcription errors in synthetic proper names; neither caused an unconfirmed send. They also reproduced a real lookup defect: Codex’s `searchTerm` returned no match for `shopping list` but found `Shopping list`. The adapter now pages bounded metadata and filters titles without case sensitivity. A regression test failed before the fix and passed afterwards; duplicate case variants across pages and exhausted scan budgets are also covered. Requests still return at most ten matching titles, and cursors continue the bounded search. The direct-control uniqueness check examines at most 1,000 titles and asks for confirmation if the budget is exhausted. The action tool description now explicitly excludes routing words from the message while preserving the target instruction verbatim.
