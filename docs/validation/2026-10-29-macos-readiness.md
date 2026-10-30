# macOS release checks — 29 October 2026

This is an unattended build and contract review. No app was launched or restarted, no microphone or speaker was used, and no account credentials or personal memories were inspected. It does not repeat or replace the earlier human voice checks.

## Checks

- The three current updater tests pass: the real Tauri downloader accepts the signed fixture, rejects tampering and version replay; package addresses must match the version and platform; deferred versions and polling times survive reopening the journal. These tests never replace an installed app.
- All ten Python contracts, the website structure check, version agreement for `0.1.0`, and the microphone plist checks pass. The first direct Python module invocation used the wrong import path; the repository's documented discovery command passed.
- The existing QA bundle passes strict signature verification and declares the microphone entitlement. That is development-bundle evidence, not public distribution approval.

## Packaging finding

The old production bundle in `target/release/bundle/macos/notdot.app` was built on 27 October at 04:33. Its executable SHA-256 was `8e0132a8980e4e36a319cf4282672a8a55d681b82aeea06582fbea8d12719244`. Its signature was internally valid, but it had no microphone entitlement and was missing the newer Silero/Whisper wake models and their licences. Its resource manifest differed from current source. It cannot be used as the current release candidate.

A fresh production bundle was built with `CARGO_BUILD_JOBS=2 cargo tauri build --bundles app`. The normal unsigned build initially had only the linker signature, so it was explicitly sealed with a local ad-hoc runtime signature and the source microphone entitlement. Strict verification then passed. The app has the exact microphone entitlement, microphone explanation, arm64 executable, macOS 14 minimum, production frontend without the debug composer, and byte-for-byte agreement with all 23 source wake resource files. No updater private key was read. The old updater archive and signature were moved to ignored `.local/macos-readiness/stale-20261027/` so they cannot be confused with this candidate.

The final follow-up build on 30 October includes source freeze `dede90f`, including the shared wake engine and updater handoff fixes, and completed in 1 minute 23 seconds. The locally ad-hoc-sealed app is 93.70 MiB. Its executable SHA-256 is `7016d7d9e919984d9dfafd66348ddd111656b8f3ba57cd473dfec51380fa82a1`. The same integrity, entitlement, architecture, frontend and 23 resource checks passed after this rebuild. Dynamic dependencies resolve only to Apple system frameworks and `/usr/lib`. This candidate has not been launched; no current updater archive/signature was generated.

## Remaining release gates

1. Rebuild and identify every release artifact from the agreed release revision. Do not reuse an older updater archive just because the version number is the same.
2. Configure Developer ID signing and notarisation, or explicitly choose an unsigned distribution policy before public macOS downloads. The old ad-hoc bundle is rejected by Gatekeeper; a passing `codesign --verify` alone does not establish public launch approval.
3. Run installed-app sign-in startup and with-Codex watcher acceptance in a disposable OS account. QA intentionally blocks those OS changes.
4. Exercise installed update consent, download/progress, replacement, relaunch and sole-writer recovery with the release candidate. Fixture signature tests do not prove these actions. No public feed exists yet, and publishing remains unauthorised.
5. Repeat current-candidate human microphone, cancellation, wake, permission-denied/device-loss, lock-screen privacy and keyboard/VoiceOver checks. Earlier dated reports identify the builds actually exercised. Broader names, accents and background noise remain experimental wake gates.

Local build, signature and contract logs are under ignored `.local/macos-readiness/`. No demo recording, public release, tag, merge or GitHub Actions change was made.

## Silent credential restoration audit

A source review found that macOS silent key reads used the production service name directly, while explicit access correctly selected the separate QA Deepgram service. If both entries existed, QA startup could use the production Deepgram key; if only the QA entry existed, silent startup could incorrectly appear unconfigured. This was a policy mismatch found without reading either credential, not evidence that a production key was actually used.

Silent search now derives its service/account from the same policy as explicit access. Production identity and the intentional read-only QA reuse of the ElevenLabs key stay unchanged. No key is moved, copied or migrated. A regression failed with the old hard-coded service and passes with the shared policy for QA/production and both providers. All five credential tests pass, including cache replacement, denial and QA mutation protection. OS credential access for the rebuilt app is not exercised unattended.

The source-only follow-up found no additional release-blocking issue in the checked resource permissions or key-readiness guards. Bundle directories/executable are `0755` and data files `0644`; both observed app-data directories are `0700`, which startup also enforces on Unix. These were metadata checks, without opening stored memories or credentials. Provider/model text uses text nodes; the only `set_inner_html` call mounts fixed debug-only markup. A non-blocking UI issue remains: the shared Settings message area can retain an old backend error after the backend clears it, until another local message replaces it. Clearing that needs to preserve newer validation messages rather than clearing on every empty snapshot.

## Shared wake regression follow-up — 30 October

At `dede90f`, 53 native app tests, four shared-runtime unit tests and three helper lifecycle tests passed. Seven other native/private-fixture tests and the full-model helper case were explicitly ignored in that default invocation. The full-model case was then run separately and passed all five positive/negative fixtures through one persistent generic Mac helper. This is offline local model computation, without microphone, playback, STT/TTS or account access. Native/shared-runtime all-target Clippy passed with warnings denied. Build jobs and test concurrency were limited to two.

The new release handoff rejects a wake-triggered capture, waits for worker cleanup, and fails closed if helper reaping is unconfirmed, including startup/cancellation paths. A per-child confirmation latch and sticky process registry prevent later success from erasing an earlier cleanup failure. Installed update replacement remains unverified.

The earlier `9cb7609` production candidate had separate keyboard/Settings checks, but its voice service defaulted to ElevenLabs and QA Deepgram credentials are separate. Those UI checks do not establish a current production Deepgram voice pass. The earlier genuine QA voice results remain historical, tied to their dated reports. No current-candidate live voice claim is made.

## Unattended dependency and installer checks — 30 October

`cargo-audit 0.22.1` checked the locked workspace using RustSec database commit `ee1a49a2769a24a596afc1c20cb33748d2769586` (updated 29 October). It reported zero vulnerability advisories, six unmaintained-dependency notices (`fxhash`, `instant`, `proc-macro-error`, `zbus_names`, `zvariant`, `zvariant_derive`) and the existing GLib unsoundness warning. The locally backported GLib provenance remains covered by the existing vendor checks; warnings were not suppressed. GLib is absent from the Mac normal-runtime Rust dependency closure. This scan does not cover native bundled libraries or unknown vulnerabilities.

An ignored inventory records 350 packages in the Mac normal-runtime Rust dependency closure. Every third-party package has a declared licence expression. The inventory exposed missing first-party package metadata: declaring `workspace.package.license` did not make members inherit it. All five notdot manifests now explicitly inherit Apache-2.0; `cargo metadata` verifies the result. This does not replace the remaining complete binary notice inventory, including frontend, native libraries and redistributed platform components.

A separate **filesystem installer integration** exercised the actual pinned `tauri-plugin-updater 2.14.0` installation path on this Mac. `UpdaterBuilder::executable_path` pointed only into a synthetic temporary `Fixture.app`; `restore_update` avoided all network requests. An ephemeral fixture-only signing key signed the test payloads and was then deleted, together with the generation log, before the installer probe ran. No production signing key, OS credential or installed app was used.

All four checks passed:

- Tampered package bytes failed signature verification and left the old fixture unchanged.
- A manifest announcing a different version failed signed-version verification and left it unchanged.
- A correctly signed malformed archive failed extraction and preserved the old fixture.
- A correctly signed synthetic archive replaced the temporary bundle through the actual macOS installer, removed its obsolete resource, installed the new executable/resource bytes, and retained `0755` bundle/executable permissions. Forged cached extraction-path metadata did not affect an outside sentinel.

The mock Tauri host supplied configuration, but the verifier and filesystem installer were the real dependency code. The synthetic executable bytes were never run. This closes that bounded local installer boundary only: it does not establish the in-app consent/download UI, large-package behavior, live production-key verification, installed-app restart/sole-writer recovery, publisher identity, notarisation, or OS-protected installation. Those remain release gates.

Ignored evidence: `.local/macos-readiness/dede90f-audit.json`, the Rust licence inventory, `license-metadata-after.json`, `installer-probe/`, and `installer-probe-results.jsonl`. Only fixture public material and signatures remain; no fixture private key persists.
