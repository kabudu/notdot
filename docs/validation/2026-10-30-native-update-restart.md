# Native installed update and restart — 30 October 2026

The final macOS native fixture passed a signed runnable bundle update from **9.9.8 to 9.9.9**, followed by a real Tauri/LaunchServices restart. This closes a stronger boundary than the earlier filesystem-only probe; it is not a full notdot updater acceptance claim.

| Check | Observed result |
| --- | --- |
| Signed malformed archive after writer shutdown | Actual updater rejected installation; actual Bridge reconnected to a working fake RPC writer. |
| Signed runnable update | Actual pinned updater replaced the private installed `.app`; its bytes match the prepared signed replacement and its signature remains valid. |
| Restart | Real `AppHandle::restart()` launched the updated 9.9.9 process through LaunchServices. |
| Profile preservation | Synthetic association/profile bytes stayed unchanged outside the replaced bundle. |
| Sole writer | Three writer processes across failure recovery and successful restart; maximum concurrent owners of an exclusive OS file lease was one. |
| Single instance | An additional updated executable was rejected; the surviving instance retained its working writer. |
| Cleanup | All observed fixture PIDs exited. The disposable signing private key and its generation log were deleted before launch. |

The final evidence is `.local/macos-update-acceptance/run-ikf6gvy2/summary.json`, with ordered process events, build logs and signed public fixture material alongside it. Installed executable SHA-256: `17acadbc251bee36a348141b8b9a90960b3d1d87841996dfb13d9da850f33ab4`. The fixture uses Tauri 2.12.5, updater 2.14.0, single-instance 2.5.4 and the repository's real Bridge implementation. Its unique identifier is `com.kabudu.notdot.update-acceptance-fixture`; production/QA apps, profiles, credentials and signing configuration were untouched. No window, microphone, provider or real Codex account was used.

The source is retained under `scripts/update_acceptance_fixture/`; `scripts/run_macos_update_acceptance.py --run-native-fixture` reproduces the bounded test. An asynchronous duplicate callback has a two-second observation bound rather than assuming notification was processed when the sending process exits. Formatting and fixture Clippy with warnings denied passed.

## Actual notdot orchestration boundary

The production `updates.rs` installer was inspected: it serializes update operations and submissions, reserves voice capacity, checks active work/source, suspends the wake decoder, downloads with its existing limits, shuts down the sole bridge, installs, and restarts. On error it invalidates update consent, restores idle state and releases the submission guard before attempting reconnection when the bridge is absent. `connect()` itself acquires that same submission gate, preventing a manual reconnect from entering during installation. No production defect was established by this check.

A new focused **release-mode** test executes the real production `Updates::fail` and journal reload. It verifies that failure clears spoken consent and prompts, retains the failed version/error, preserves polling/deferred-version state across reload, and does not restore expired authorization. It passed; it does not execute the whole installation controller.

The native fixture uses the same underlying components and shutdown/recovery order, but it does not call the private `AppState` installer. Production consent/download UI, release URL validation during a deployed update, production-key verification, genuine Codex conversation recovery, protected installation and other-platform installers remain separate acceptance boundaries. No bypass for those production policies was added.
