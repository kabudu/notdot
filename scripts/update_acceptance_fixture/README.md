# Native macOS updater fixture

This opt-in, no-window accessory app tests a real installed bundle update without opening notdot or using an account, microphone, voice provider or OS credential store.

From the repository root, on an unlocked Mac desktop:

```sh
python3 scripts/run_macos_update_acceptance.py --run-native-fixture
```

The runner builds synthetic versions 9.9.8 and 9.9.9, installs only into a private `.local` lab, creates a disposable signing pair, and deletes the private key and its generation log **before** launching the fixture. Production signing configuration is never read or changed. Existing notdot and QA apps are not launched, stopped or replaced.

The native fixture uses pinned Tauri/updater/single-instance dependencies and the actual `notdot-core::Bridge`. Its child is a fake stdio RPC writer, guarded by an exclusive OS file lease. It first closes that writer, rejects a validly signed malformed archive, reconnects and checks the synthetic profile. It then installs a signed runnable replacement, calls the real Tauri restart, verifies the new version/profile, rejects a duplicate instance and shuts down. Timeouts bound the process and duplicate checks. The runner checks that all observed fixture processes exited.

This deliberately does **not** invoke notdot’s private `AppState` installer or its consent, download-budget and release-URL controller. Those remain separate application-level checks. The fake association is not a real Codex conversation. No public release, production signing-key verification or OS-protected installation is claimed.

The fixture is a separate workspace with intentionally synthetic versions. Its lockfile pins the test graph; update it when the path dependency changes. The normal app workspace and production release version are unaffected.
