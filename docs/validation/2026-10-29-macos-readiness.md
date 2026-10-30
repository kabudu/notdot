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

## Current production package — `3ca1b45`

The newly built normal macOS app (`com.kabudu.notdot`, version 0.1.0) passed strict deep signature verification. It is **locally ad-hoc signed**, arm64, requires macOS 14+, and has the microphone usage description and exactly the `com.apple.security.device.audio-input` entitlement. This is not Developer ID signing or notarisation.

All **23 wake resource files** and **322 notice-bundle files** match the current source bytes, with identical file inventories. Bundle size is 101,117,363 bytes. The executable SHA-256 is `ac2ca0e555a207047252e044914ac0229a71f7a55c3390eb2127efea24170cff`; detailed hashes are retained in `.local/macos-readiness/3ca1b45-bundle-verification.json`.

This was a file/signature check only: no app launch, UI, microphone, playback, provider or credential access, and no broad test rerun. Current production live-voice acceptance remains open. The coordinator's separate optimized QA instance is awaiting an observed local saved-key approval; approvals are being held for the user's return, and no new approval was requested by this check.

## Window-restore package refresh — `40ce478`

The four reviewed window-restore additions were packaged into both Mac acceptance artifacts without launching them. Both are version 0.1.0, arm64/macOS 14+, locally ad-hoc signed, pass strict deep signature verification, retain exactly the microphone entitlement, and match all 23 wake-resource plus 322 notice-file source hashes.

| Artifact | Identifier | Executable SHA-256 |
| --- | --- | --- |
| `target/release/bundle/macos/notdot.app` | `com.kabudu.notdot` | `4cdd18d952ace35c9deb7fad5fe0841e68375bf09e3291c83b5d5a047b04dc2b` |
| `target/release/bundle/macos/notdot QA Final.app` | `com.kabudu.notdot.qa` | `55f830a175bf0d29de18cf367eca5b7366cafbabf233862d0a116ea43f1fba51` |

The original running `notdot QA.app` executable is unchanged from its preserved fingerprint. The separate QA Final build also left the prepared production bundle unchanged. No UI, audio, credentials, approval requests or native voice tests were used. The unlaunched QA Final artifact is prepared for the coordinator's batched local approvals when the user returns; current Mac live-voice acceptance remains open. Detailed checks are in `.local/macos-readiness/40ce478-production-verification.json` and `40ce478-qa-final-verification.json`.

## Final QA launch and ordinary UI — `40ce478`

The coordinator subsequently quit the obsolete running QA app through its own menu and launched the separate `notdot QA Final.app` above. It launched responsively without requesting saved-key access. Settings exposes the current collapsed sections, Terra Medium summary and the revised Deepgram/ElevenLabs guidance, including that an existing key does not need re-entering. No key-access, preview or microphone control was activated; no settings were saved. Wake listening correctly remains paused until saved-key access is authorised. Current Mac voice acceptance is still pending the owner's return for grouped local approvals.

The app menu exposes About, Settings, Check for Updates and Quit. About displays the logo and version and dismisses with Escape. However, the native Computer Use accessibility snapshot exposes no About contents while this dialog is open, both on its own and over Settings; closing it restores the normal accessible tree. This is an unresolved accessibility observation, not a screen-reader acceptance pass or a proven cause. It needs bounded follow-up before claiming complete accessibility coverage. No additional rebuild or OS approval was initiated for this observation.

## Current Mac voice acceptance — `40ce478`, 30 October

After local approval, the same QA Final build reported both “Deepgram is ready” and “Your saved key is ready.” No rebuild, key re-entry, key export or provider change was used. Settings remained Ava, wake enabled, Deepgram recognition, Bella replies and Terra Medium. These tests used generated speech played through the Mac speaker into its microphone; no demo recorder was running.

- Three arithmetic turns transcribed “What is fourteen minus six? Please answer briefly.” exactly and returned “Eight.” All completed in the existing QA chat.
- A longer compiler request was transcribed exactly, produced the requested ten-sentence public answer and entered speaking. The orb showed smooth circular shell bands rather than radial spikes. Escape stopped playback, removed the speaking shell and returned to wake listening; a subsequent request succeeded.
- Thinking showed the moving circular arc, followed by the speaking shell. On one recovery turn, speaking was observed 2.303 seconds after the generated request ended; on the later recovery turn, 3.502 seconds. These are individual UI-observed timings including polling, not a benchmark or guaranteed latency.
- An early cancellation during preparation created no new model turn. A separate cancellation after model submission persisted the turn as **interrupted**, with no final-answer item. The latter transcript added an initial “and” to the otherwise correct compiler request; recognition was not perfect. Escape-to-follow-up-snapshot took 809 ms including Computer Use input/snapshot overhead; this is not an isolated runtime latency measurement.
- After that model interruption, the next hands-free arithmetic turn traversed recording → thinking → speaking → cooldown → wake listening and completed correctly. The current public turn count advanced from 22 to 27 across this pass: four completed model turns and one interrupted turn.
- A left click on the orb in wake mode did not activate recording. The bare-name “Ava” fixture also left the UI in wake listening during the observed check; that single negative example does not establish ambient false-trigger reliability.

The first manually sequenced fixture was mistimed by the harness: it activated wake listening but the request arrived too late, with no new model turn. It was excluded from successful turns; subsequent fixtures scheduled the request one second after the wake sample. A bounded read-only oracle checked only the exact QA chat, public synthetic user/final-answer items and turn status; reasoning, tools, credentials and unrelated chat contents were not printed.

Exact synthetic turn IDs: first subtraction `01a18b35-6522-7d92-b3ad-2bf25115198a`; compiler/playback cancellation `01a18b35-eebf-71d3-834d-480bad03508c`; first recovery `01a18b37-1661-7352-9274-93cb9c726367`; interrupted model `01a18b39-15bc-7fd1-935b-3f9a95dbe85a`; final recovery `01a18b39-dbfd-7d03-95c0-8ecf9b0e7204`. Ignored fixtures and the bounded oracle are in `.local/macos-readiness/final-voice/`.

This closes the current synthetic Mac voice/cancellation pass. Fresh human hearing, pop/noise assessment, wider natural speech, device-loss coverage, installed update/restart and the About accessibility observation remain distinct acceptance work. No rectangular focus border appeared in these captures, but Tab did not establish a positive keyboard-focus-cue pass in the current macOS navigation configuration. No source changed, so the existing `40ce478` full local CI remains applicable.

The startup switch was also exercised in the isolated QA profile: Save correctly refused OS startup changes with “OS startup changes are disabled in the isolated QA profile.” Closing Settings discarded the temporary toggle; persisted startup remained manual and no notdot login agent was created. This verifies the QA safety boundary, not production Mac login registration or actual sign-in launch.

## Prepared universal Mac candidate

A separate-target build from `3ddee56` produced a normal **arm64 + x86_64** app without replacing or launching either active QA app. Both slices declare macOS 14.0, contain native sherpa/Whisper symbols and retain exactly the microphone entitlement. Strict deep, all-architecture ad-hoc signature verification passed; all 23 wake resources and 322 notice files match source hashes. Both native build caches confirm `GGML_NATIVE=OFF` and deployment target 14.0. macOS 15 is the planned CI runner image, not a raised deployment floor.

- App: `.local/macos-universal/target/universal-apple-darwin/release/bundle/macos/notdot.app`; 155,550,403 bytes.
- Executable SHA-256: `c98bf01ed1c6b962be2cb7dabf0cfa0207c2b7bafae8adc65a432901367905e3`.
- Prepared unsigned updater container: `.local/macos-universal/notdot_0.1.0_darwin-universal.app.tar.gz`; **71,594,877 bytes**, within the 256 MiB download budget; SHA-256 `04b6ccd427eb0dd7d85e2534c10c05688a8a23882c0f8d5eb665bb45427e8c56`.

This is a locally ad-hoc signed app, not Developer ID/notarised release acceptance. A genuine x86_64 CPU-only Mach-O probe failed with “Bad CPU type” on this host, so Rosetta execution is unavailable. Dual-slice structure is verified; Intel execution and Intel hardware voice are not claimed. No app was launched, no key/approval was requested, and no fresh live-voice pass is attached to this build. Detailed hashes and per-slice checks are in `.local/macos-readiness/universal-verification.json`.
