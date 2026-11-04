# Local Codex chat access and wake recovery

## Scope

Changes on `codex/chat-access-wake-reliability`, following the 0.2.0 release. Testing used the Mac host, its authenticated local Codex runtime, Terra Medium, the bundled wake models and synthetic fixtures. The installed 0.2.0 app and its saved voice keys were not replaced or modified during implementation. No new QA app was installed.

## Chat access

- The built release executable's helper entry point passed live initialization, tool discovery, chat lookup/read and rejection of an attempted write method, without starting another orb or accessing voice keys.
- Live MCP protocol checks initialized the bundled service, listed its two tools, found ten local chats and successfully read a page of messages. Private message content was not printed or committed.
- A separate model test created a synthetic named source chat containing a random marker. An ephemeral Terra Medium chat received the lookup service and a prompt that contained the title but neither the marker nor the source chat ID. It invoked both `find_chats` and `read_chat`, then returned the correct marker. The synthetic source chat was archived afterwards; the ephemeral chat was unloaded.
- Fake-runtime integration tests check filtering of reasoning, images, tool output and filesystem metadata; invalid IDs, oversized arguments, unknown tools, UTF-8 truncation, pagination and both new/resumed companion registration. These are protocol tests, not voice tests.
- The service uses only `thread/list` and `thread/turns/list`. It does not resume, interrupt, write to, archive or take ownership of target chats. The source-chat archive above is test cleanup performed by the test driver, not a service capability.

## Wake recovery

A regression test reproduced a concrete failure: one backup recognizer error caused `Worker::frame` to enter a permanent error state before the independent primary detector could process another frame. The original code failed the regression assertion. The fix keeps the primary detector running, reports the backup warning and permits another bounded backup decode on subsequent speech. Successful recognition clears the warning.

The expanded wake suite passed 31 tests, with five explicit diagnostic/environment tests ignored. Added cases include a previous backup failure, three positive wake recordings and two negative recordings at 2%, 5% and 20% amplitude, plus the same five recordings with low-frequency rumble and input bias. Existing extended-idle, 48 kHz, cancellation, microphone queue and speech-boundary tests also passed.

These tests do not establish the cause of the single missed acoustic activation in the installed 0.2.0 test. They establish the separately reproduced failure and its recovery. Nor do they establish all-name, all-accent, room-distance or cross-platform microphone accuracy. The real installed-app voice exchange recorded earlier remains a 0.2.0 result, not proof of this changed build.

## Checks

`scripts/ci-local.sh` passed: release-mode workspace tests, native and WASM Clippy (including the debug-only UI feature), format checks, gesture/visual/speech-queue checks and 99 Python checks. Live chat checks used the authenticated runtime without API-key exports or Keychain access. See the PR checks for Windows and Linux build results.
