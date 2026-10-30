# Desktop release readiness

This page tracks the work needed before publishing a release. Items left open are still required. It is a developer checklist, not a list of promises that every feature is already verified.

The owner authorised the initial 0.1.0 public release on 30 October 2026. GitHub Actions is enabled. The repository is still private because GitHub retains retired audio in read-only references for closed pull requests 1–4 after every normal branch was cleaned. Publication is waiting on that repository-history issue; no hosted release build or public binary has been published. The repository identity, all eight signing secret names and the current demo attachment are preserved. Local validation passes, but it is not a hosted-CI result.

The release workflow extracts the dated version entry from CHANGELOG.md and creates a draft GitHub release with the selected installers, updater files, checksums and required source/provenance assets. Publishing the validated draft makes the initial release and update feed public. The committed release-notes copy must match the changelog entry.

## Current candidate

The voice backend remains the one tested at `3ca1b45`; later changes improve saved-key guidance, window restoration and universal Mac updater addresses without changing voice processing. A universal Mac app now contains both Intel and Apple Silicon slices, targeting macOS 14+. Native update/restart fixtures pass on Mac, Windows and Linux. These are private test candidates, not published releases.

| Platform | Verified | Still open |
| --- | --- | --- |
| macOS arm64 | Current package identity, strict ad-hoc signature, microphone entitlement and resources; hands-free microphone → Deepgram → Terra Medium → speech; thinking/speaking feedback; playback and active-model cancellation with recovery | About accessibility needs verification; wider natural-speech/device coverage, publisher signing and installed-update tests remain open |
| Windows x64 | 55 native tests; helper lifecycle and wake corpus; two consecutive hands-free microphone → Deepgram → Terra Medium → speech turns; playback cancellation; minimized-window restoration; startup registration on/off | Wider natural-speech and device-loss coverage; installed-update and sign-in startup acceptance |
| Linux arm64 VM | Normal package renders without special flags; one physical wake turn; controlled virtual-input speech, negative wake examples, playback cancellation and recovery; window restoration; startup registration on/off | Natural-voice, device-loss and actual sign-in startup acceptance remain open |
| Linux x64 | Build and isolated package checks | Genuine hardware voice and installed AppImage update acceptance remain open |

The temporary Mac keep-awake guard and Linux idle overrides have been removed after local testing. Temporary Linux audio routing and Windows name/wake settings have been restored. The owner approved both saved-key reads on the final Mac QA build, and the current synthetic voice pass is complete. Generated speech clips establish the exercised paths, not broad recognition accuracy or human hearing. Detailed results are in the [Mac](validation/2026-10-29-macos-readiness.md), [Windows](validation/2026-10-29-windows-scenarios.md) and [Linux](validation/2026-10-29-linux.md) reports.

The complete local validation script passed at `40ce478`: 133 Rust tests, 16 Python checks, formatting, Clippy for the native backend and both UI configurations, and the offline notice check. Thirteen tests requiring explicit native/provider fixtures were skipped by this offline run; platform reports record the separately executed live checks.

## Runtime and startup

notdot owns a standalone Codex app-server child process and its persistent conversation. It does not attach to the desktop GUI's live runtime and does not require its window in front. The installed runtime and valid account sign-in remain dependencies. Its backend is Codex; changing the desktop app to ChatGPT Work or Chat does not switch notdot's engine or provide a Work/Chat conversation bridge. Read-only history viewing is the verified shared use.

Official current documentation lists desktop installations for macOS, Windows and Linux: https://learn.chatgpt.com/docs/app and https://learn.chatgpt.com/docs/linux/linux-app. Availability does not prove notdot compatibility. Each platform's runtime/account/tools need a live acceptance test. `NOTDOT_CODEX_BIN` can select a native Codex executable; shell command shims such as codex.cmd are not a substitute for codex.exe on Windows. notdot never installs or redistributes the OpenAI runtime automatically.

Startup defaults to manual. Sign-in startup is opt-in and uses Tauri autostart. With-Codex mode starts a hidden watcher at sign-in, scans process names/paths every five seconds and activates the companion when the desktop process is seen. A helper must already be running to observe another app's launch. This is not an OS facility that starts notdot out of nothing when Codex opens. Manually opening notdot always activates it.

OS autostart can be disabled outside the app. macOS background desktop launch uses `open -g`; launching it is optional because the bridge operates independently. Automatic desktop launch on Windows/Linux is not implemented yet and must not be advertised.

## Updater contract

- Ordinary network check at most every six hours while enabled and running, with a persistent last-attempt time; manual checks are available in Settings. No model calls for discovery. Deferred versions remain deferred across restarts.
- The feed is the fixed GitHub latest-release asset `latest.json`. There is no public feed yet; private unauthenticated checks cannot fetch it. No GitHub tokens are embedded in the app.
- Only a newer stable version reported by Tauri's comparator is offered. The package URL must use HTTPS and the matching `kabudu/notdot/releases/download/vVERSION/` path. Artifact signatures must bind the advertised version. Tauri verifies before installation; no signature bypass is available.
- Local system speech announces an available update once the companion is idle. A bounded twelve-second confirmation recording uses the existing STT provider and stops after detected speech plus silence. Only explicit allowlisted affirmative replies within thirty seconds grant consent for that exact version. Unknown replies defer. A failed/missing voice provider leaves the manual install control available; it cannot provide hands-free STT without a working STT service.
- The update announcement uses macOS system speech, Windows System.Speech, or Linux speech-dispatcher's `spd-say`, independently of paid TTS quota. Microphone permissions and OS speech availability are required.
- Downloads have a five-minute timeout and 256 MB budget, expose byte progress, and serialize against checks/installation. Active model work and voice provider calls block installation. The app closes its sole writer before replacing files. It restarts automatically on macOS/Linux; the Windows passive NSIS updater handles relaunch.
- Public replacement/restart, interrupted downloads, filesystem errors, read-only installations, runtime ownership after restart and hardware spoken consent still require installed-platform acceptance. Signature fixture tests do not establish those outcomes.

## Keys and releases

The production updater public key is committed in assets/updater.pub and the Tauri configuration. Its private counterpart was generated locally outside the repository in the owner's private release-data directory, with owner-only permissions. Back it up securely before shipping. No private key is committed or placed in an app. Both matching TAURI_SIGNING_PRIVATE_KEY and TAURI_SIGNING_PRIVATE_KEY_PASSWORD repository secrets are provisioned. The existing key was retained, not rotated.

Updater signing is separate from publisher identity signing. Windows first release may be Authenticode-unsigned, as requested, while still requiring updater signatures. The owner supplied the Apple repository secrets, and the release workflow uses them for Developer ID signing and notarization with verification of the app and its actual updater archive. Hosted signing has not run while Actions remains disabled. Current local Mac artifacts are development builds, not a publicly cleared distribution.

The prepared manual workflow is additionally gated on a public repository and PUBLIC_RELEASES_ENABLED=true. It builds a universal Mac app and disk image plus native Windows/Linux packages, creates a complete version-bound manifest and checksums, and only creates a draft release against the exact matching existing tag. Both Mac updater targets use the same universal archive. Linux publication requires the matching corresponding-source companion; Debian packages are staged too. It does not automatically publish that draft. Release notes, version agreement, complete artifact set and installed-platform validation are required before public opening. scripts/prepare_release.py stages files without publishing. This follows the build, signing, immutable-tag and verification boundaries reviewed in kabudu/lascoria's release workflow. The local lascoria-web checkout is a separate website repository.

## Platform acceptance gates

- [ ] macOS: branded installed app, startup registration, watcher activation, About, microphone, headless Terra/tool turn, spoken update consent, replacement and relaunch.
- [ ] Windows x64: unsigned current-user NSIS install, WebView2, native Codex discovery/sign-in, keychain, microphone/audio over remote desktop, startup, updater replacement and relaunch.
- [ ] Linux x64: Debian 12 / Ubuntu 22.04 compatible build, WebKitGTK 4.1, Secret Service, speech-dispatcher with an installed speech engine, media codecs, microphone, native runtime and installed AppImage update/restart. Debian packages require system dependencies and may require package-manager privileges; hands-free updates target the user-owned AppImage.
- [x] Universal Mac packaging: both architecture slices built and verified, with the same updater archive offered to Intel and Apple Silicon. Prepared CI validates on both native Mac runner architectures. Intel execution was not established locally because Rosetta is absent; hosted execution remains pending. The release matrix is universal macOS, Windows x64 and Linux x64.
- [ ] Public assets, release notes, provenance, license inventory and anonymous manifest/download verification.

Run scripts/ci-local.sh before committing delivery changes. Do not mark unchecked platform gates complete based only on cross-compilation.

## Local preparation evidence

- The Windows NSIS installer is locally built without Authenticode, installed through Windows App and visually exercised. A real one-click voice turn was audibly confirmed, and model/playback cancellation checked; [the validation report](validation/2026-10-29-unattended.md) separates these passes from remaining release gates. About shows the same logomark as the taskbar icon. A later local candidate adds stricter package-address validation; keep installer candidates matched to the source revision when testing updates.
- macOS app/updater archive and Windows updater signatures are generated locally with the committed production public key's private counterpart. No release assets have been uploaded.
- Debian 12 Linux x64 compilation, `.deb` installation and isolated desktop, credential-store and virtual-audio checks pass. The latest AppImage is 197.4 MiB, below the updater’s 256 MB bound, with bundled GTK/GStreamer and verified resource checksums. Each format was rebuilt from a pristine executable so its bundle marker is correct. The Debian package now explicitly requires a speech engine for system alerts. Native x64 wake, genuine voice/model and installed AppImage update/restart acceptance remain open. See [Linux validation](validation/2026-10-29-linux.md).
- The local x64 Linux packaging tool's static-PIE AppImage launcher cannot execute under this Mac's emulation. Extracting its SquashFS and using the vendor AppRun fixes tool execution. Native Linux public runners should not need this Mac-only workaround. `scripts/linux.Dockerfile` records the Debian build prerequisites; runtime acceptance needs actual desktop/audio access.

The final Mac launch trial exposed a saved-key read blocking the main thread in macOS Security. Startup now checks only key-entry metadata on a blocking worker, without decrypting the secret. Voice use retains the normal OS credential authorization. The corrected app launches responsively, preserves Ava/hue 195/Bella, and its About screen was visually verified. Local ad-hoc signing seals the development bundle; strict codesign verification passes, without claiming Developer ID or notarization.


Wake feature release gate: the new native microphone path needs installed-device permission, unplug/stall, ambient-noise/false-trigger and speaker-echo tests on each OS. The openly licensed five-clip wake corpus passes three timing variations plus bare-name and unrelated-speech negatives; its provenance describes the single-voice coverage. This does not establish broad hardware or accent reliability. Keep wake listening opt-in and experimental until user/accent acceptance is established. New bundles include approximately 47 MiB of model/tokenisation/legal resources, including the Whisper wake fallback; checksum verification runs at build time. Rebuild all platform packages from the release revision, audit complete native static-library notices and replace/clear rights for private synthetic test fixtures before public distribution. Earlier local release-preview packages do not contain wake detection.

## Optional Windows wake helper gate

The optional persistent wake helper preserves the generic app's CPU minimum. It is selected only after the complete CPU/OS guard and compiled-in package digest pass; its model checksum, protocol limits, three-second decode deadline and cancellation cleanup remain local. Build the helper in its separate cache before the generic Windows app. Do not carry helper-specific compiler settings into the app cache. Generated helper files stay untracked; the normal Windows resource map packages them only when prepared.

Before accepting a candidate with this resource, verify the installed helper hash and path, `fallback_decoder=accelerated`, all five positive/negative full-context fixtures through the persistent worker, actual wake activation, cancellation with no lingering helper, and missing/tampered/unsupported selection falling back without an unsafe launch. Initial setup is separately bounded before microphone capture; helper failure does not trigger a synchronous second decode of the same utterance. Package completeness, native installed-device tests and total wake latency remain release gates even when component tests pass. No public CI, publication, hardware-minimum change or new provider is enabled by this implementation.

The generic Windows full-context decoder remains too slow for the production three-second deadline on the tested laptop (about nine seconds with four threads). Its full five-fixture correctness was checked separately with a fifteen-second diagnostic budget. Do not count that diagnostic as live wake acceptance: production acceptance must verify the packaged accelerated backend and the complete microphone path.

## Installed update and universal packaging evidence, 30 October

Mac and Windows disposable fixtures exercised the actual pinned updater, installed replacement and automatic restart, real Core Bridge shutdown/reconnect, duplicate-instance rejection, one active writer and preserved synthetic profile bytes. Signed-malformed packages failed and recovered before successful updates. These are native component lifecycle checks, not claims that notdot's public GitHub feed or spoken consent flow has run against a published release. See the dated platform reports. The native Linux ARM64 AppImage fixture also passed malformed-input and read-only-install recovery, real replacement/restart, preserved profile bytes and single-writer ownership.

The universal Mac app passed strict signature verification for both slices and contains matching wake/notice resources; its 71,594,877-byte updater transport is within the 256 MiB limit. It remains ad-hoc signed locally. The public release guard rejected it at the Developer ID boundary, as intended. Prepared publishing verifies Developer ID/team, both slices, stapled tickets, the exact updater archive, and the actual mounted disk image. Hosted signing/notarization stays deferred until the owner makes the repository public and enables Actions. No Apple credential was exported from GitHub or requested in chat.

## Public test assets and distribution records

Current wake-test clips use openly licensed Piper synthesis with retained model/tool provenance; they replace the private-development system-voice recordings. The native helper passes all five replacement clips with the same model, production settings and negative expectations. CI checks audio/source hashes and runs this offline native model case on both prepared Mac runner architectures. Historical private development assets are not approved public release inputs; keep that history private when preparing the public source export.

Windows-only installer originals and AppImage runtime/platform notice copies supplement the locked Rust/native index. The AppImage builder prepares a conservative set of original distro copyrights before bundling; the source collector verifies the exact selected package subset and all embedded runtime/common terms. This changes attribution resources, not voice processing.
