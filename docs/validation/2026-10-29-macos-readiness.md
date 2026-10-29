# macOS release checks — 29 October 2026

This is an unattended build and contract review. No app was launched or restarted, no microphone or speaker was used, and no account credentials or personal memories were inspected. It does not repeat or replace the earlier human voice checks.

## Checks

- The three current updater tests pass: the real Tauri downloader accepts the signed fixture, rejects tampering and version replay; package addresses must match the version and platform; deferred versions and polling times survive reopening the journal. These tests never replace an installed app.
- All ten Python contracts, the website structure check, version agreement for `0.1.0`, and the microphone plist checks pass. The first direct Python module invocation used the wrong import path; the repository's documented discovery command passed.
- The existing QA bundle passes strict signature verification and declares the microphone entitlement. That is development-bundle evidence, not public distribution approval.

## Packaging finding

The old production bundle in `target/release/bundle/macos/notdot.app` was built on 27 October at 04:33. Its executable SHA-256 was `8e0132a8980e4e36a319cf4282672a8a55d681b82aeea06582fbea8d12719244`. Its signature was internally valid, but it had no microphone entitlement and was missing the newer Silero/Whisper wake models and their licences. Its resource manifest differed from current source. It cannot be used as the current release candidate.

A fresh production bundle was built with `CARGO_BUILD_JOBS=2 cargo tauri build --bundles app`. The normal unsigned build initially had only the linker signature, so it was explicitly sealed with a local ad-hoc runtime signature and the source microphone entitlement. Strict verification then passed. The app has the exact microphone entitlement, microphone explanation, arm64 executable, macOS 14 minimum, production frontend without the debug composer, and byte-for-byte agreement with all 23 source wake resource files. No updater private key was read. The old updater archive and signature were moved to ignored `.local/macos-readiness/stale-20261027/` so they cannot be confused with this candidate.

The final incremental build includes the source freeze at `98947d2` and completed in 51.30 seconds. The sealed app is 93.69 MiB. Its executable SHA-256 is `07f0857d57ee1ae659c6a7a77cdd55b2db03b2ed945aa3ee6f18259cb204a2a3`. The same integrity, entitlement, architecture, frontend and 23 resource checks passed after that rebuild. Dynamic dependencies resolve only to Apple system frameworks and `/usr/lib`. This local candidate has not been launched; no current updater archive/signature was generated.

## Remaining release gates

1. Rebuild and identify every release artifact from the agreed release revision. Do not reuse an older updater archive just because the version number is the same.
2. Configure Developer ID signing and notarisation, or explicitly choose an unsigned distribution policy before public macOS downloads. The old ad-hoc bundle is rejected by Gatekeeper; a passing `codesign --verify` alone does not establish public launch approval.
3. Run installed-app sign-in startup and with-Codex watcher acceptance in a disposable OS account. QA intentionally blocks those OS changes.
4. Exercise installed update consent, download/progress, replacement, relaunch and sole-writer recovery with the release candidate. Fixture signature tests do not prove these actions. No public feed exists yet, and publishing remains unauthorised.
5. Repeat current-candidate human microphone, cancellation, wake, permission-denied/device-loss, lock-screen privacy and keyboard/VoiceOver checks. Earlier dated reports identify the builds actually exercised. Broader names, accents and background noise remain experimental wake gates.

Local build, signature and contract logs are under ignored `.local/macos-readiness/`. No demo recording, public release, tag, merge or GitHub Actions change was made.
