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

The earlier read-only/wake revision passed Mac, Windows and Linux CI (run 41803286376). Session-control changes require a new cross-platform run; the PR records that result. Follow-up physical speaker-to-microphone checks of the installed builds are recorded below; they are not human-listening assessments.

## Wake recovery

A regression test reproduced a concrete failure: one backup recognizer error caused `Worker::frame` to enter a permanent error state before the independent primary detector could process another frame. The original code failed the regression assertion. The fix keeps the primary detector running, reports the backup warning and permits another bounded backup decode on subsequent speech. Successful recognition clears the warning.

The expanded wake suite passed 31 tests, with five explicit diagnostic/environment tests ignored. Added cases include a previous backup failure, three positive wake recordings and two negative recordings at 2%, 5% and 20% amplitude, plus the same five recordings with low-frequency rumble and input bias. Existing extended-idle, 48 kHz, cancellation, microphone queue and speech-boundary tests also passed.

These tests do not establish the cause of the single missed acoustic activation in the installed 0.2.0 test. They establish the separately reproduced failure and its recovery. Nor do they establish all-name, all-accent, room-distance or cross-platform microphone accuracy. The earlier installed-app voice exchange was a 0.2.0 result; follow-up checks of the changed build are recorded below.

## Checks

`scripts/ci-local.sh` passed: release-mode workspace tests, native and WASM Clippy (including the debug-only UI feature), format checks, gesture/visual/speech-queue checks and 99 Python checks. Live chat checks used the authenticated runtime without API-key exports or requests for saved voice keys. See the PR checks for Windows and Linux build results.

## Installed-app follow-up

The source build replaced `/Applications/notdot.app`, using the production bundle identifier and existing Developer ID requirement. The previous official bundle is retained as an ignored rollback ZIP, not a second installed app. Both Intel and Apple Silicon slices and the signature were verified. These local builds are signed but not notarised; public release notarisation remains in GitHub Actions. Settings and keys were preserved, with Read only added as the default setting. No new saved-key prompt was needed.

Speaker-to-microphone checks exercised Deepgram, Terra Medium, ElevenLabs playback and the real orb. A spoken lookup recovered the synthetic chat’s amber marker; the follow-up window accepted “That’s all”. A separate confirmed send left the synthetic target unchanged before “Yes, send it”, then created exactly one target turn and received the expected reply. These are generated-speaker acoustic tests, not a new human-listening assessment.

Further tests exposed two transcription errors in synthetic proper names; neither caused an unconfirmed send. They also reproduced a real lookup defect: Codex’s `searchTerm` returned no match for `shopping list` but found `Shopping list`. The adapter now pages bounded metadata and filters titles without case sensitivity. A regression test failed before the fix and passed afterwards; duplicate case variants across pages and exhausted scan budgets are also covered. Requests still return at most ten matching titles, and cursors continue the bounded search. The direct-control uniqueness check examines at most 1,000 titles and asks for confirmation if the budget is exhausted. The action tool description now explicitly excludes routing words from the message while preserving the target instruction verbatim.

The installed direct-send test also reproduced an overly conservative route: the model sometimes proposed the whole recognised routing command as the target message. Scoped consent now removes only a literal routing prefix from an exact current transcript addressed to a verified unique chat. It preserves the remaining instruction and punctuation. Negations, quotations, changed commands and uncertain destinations do not gain automatic authority. The regression failed before the change and passes afterwards, alongside the existing cancellation, expiry and command-punctuation checks.

The installed source build at `c975549` passed both acoustic control paths against the real installed executable: Send when asked posted exactly one target turn without a confirmation capture; Confirm before sending left the target unchanged until the spoken approval and then posted exactly one turn. Both targets returned twelve as requested. Both final-build wake activations succeeded on the first fixture playback. The real helper/runtime smoke also passed confirmed send/reply, steering and interruption using `/Applications/notdot.app/Contents/MacOS/notdot`. A negative speech clip followed by silence left the orb idle for the full 16-second observation.

Read only was restored through Settings after testing. Every pre-existing saved setting matched the pre-install backup. Only the isolated synthetic target was archived; the companion’s history and user memories were retained. No saved-key value was exported and no new keychain approval was requested. The installed bundle’s files match the locally built universal bundle, with the obsolete official-release notarisation ticket moved to the rollback directory.

A subsequent review found that automatic matching also folded instruction case and internal whitespace. Those can be significant in filenames and code. Matching now ignores case only in the routing words and destination; the instruction preserves case, punctuation and internal whitespace, including inside quotes. A case-change regression failed before the fix and passed afterwards. The complete focused check passed 37 core tests and 18 bridge tests, with native Clippy clean.

After that review correction, source `0437240` was rebuilt, Developer ID-signed for both Mac architectures and installed in the normal app location. The acoustic direct-send check passed again: first-attempt wake activation, exactly one target turn and the expected twelve reply, without a confirmation capture. The installed-helper/runtime check again passed confirmed send/reply, steering and interruption. A fresh 16-second negative-speech/silence check stayed idle throughout. Read only and every pre-existing setting were restored, the synthetic target was archived again, and no saved-key prompt appeared. The rollback archive and installation checksums remain in ignored local evidence.

## Windows helper recovery

Run 41820253692 at source `0437240` initially missed `hey-ava.wav` after five seconds of simulated idle. The final backend was generic, with no error reported. The second failure was a poisoned fixture lock following that assertion, not an independent recognition failure. A targeted Windows retry passed; all platform jobs for that source are green. This retry does not establish the cause of the intermittent miss or a permanent fix.

A focused offline test was also prepared on the Windows laptop, without microphone, provider keys or profile changes. A generic-only launch hit the already documented slow generic warm-up limit. After correcting the harness to include the matching accelerated helper, Avast explicitly blocked that helper as IDP.HELU.PSD11. No exclusion or security bypass was applied. That establishes a local antivirus block, not the cause of the CI failure, and does not establish whether the detection is a false positive.

Code review found a concrete lasting weakness: an unavailable helper permanently changed that decoder to GenericPending and returned a normal false result. Recovery now retains the verified accelerator, reports the fault, warms a replacement without replaying the failed audio, and rate-limits failed attempts. Three-second job expiry prevents an utterance queued during recovery from activating later. Recovery metadata cannot finish a newer job or count as a wake match. Initial backup setup failure leaves primary processing active and visible warnings remain accurate.

Added checks cover real OS-process replacement and cancellation during warm-up, stale recovery metadata, expired queued audio, missing-backup setup and a Windows-only forced helper failure followed by successful recovery and failure backoff. Existing positive, negative, quiet, rumble and idle corpus assertions are retained. Fixture teardown waits for cancellation and the separate recovery warm-up; that test-only wait does not extend the production recognition deadline. Installed Windows acceptance remains blocked by the observed antivirus decision.

Recovery also distinguishes unconfirmed child cleanup from ordinary unavailability. It refuses to launch a replacement in that state, preserves the failed-release guard, and reports that an app restart is needed. Local checks: 34 wake tests passed (5 diagnostic tests ignored), 4 process-lifecycle tests passed (1 diagnostic test ignored), strict native Clippy passed, and the Windows test executable cross-compiled successfully. Windows CI execution remains the platform validation gate.
