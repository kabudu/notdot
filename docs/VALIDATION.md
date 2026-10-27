# Validation record

Environment: macOS arm64; Rust/Cargo 1.97.0; Tauri 2.12.5; Trunk 0.22.1; installed ChatGPT Codex runtime 0.164.0-alpha.11.1. No API key is stored in the repository or test evidence.

## Verified

- Real Rust bridge starts Terra Medium, completes a turn, restarts and recalls the previous answer using the same persisted thread ID.
- Native Tauri/WASM app completed a real user turn, and Desktop read the same completed history while notdot remained running. That native test used the earlier development composer; the default UI now excludes it.
- Nine core/provider Rust tests cover settings restart/validation, explicit retention intent, memory restart/deletion and dependent-summary retraction, interleaved agent events/unsupported approval rejection, runtime exit, provider bad inputs, preview-URL safety, and voice search/pagination over a local HTTP fixture.
- Native and WASM Clippy pass with warnings denied, including the opt-in WASM debug UI configuration.
- Native onboarding saves and returns to a borderless orb-only surface. The saved name and palette survive app restart.
- Live account voice catalog loads successfully. Search filters the native dropdown; more than five available voices reveals search. Voice preview API/playback path was exercised without a visible failure.
- User-reported hardware E2E passed: after selecting/saving a voice, the user tried a spoken message and confirmed “I heard a spoken reply”.
- Live provider smoke synthesized a fixed phrase (25,538 MP3 bytes) and transcribed it back as “The voice bridge is working”. This is an actual TTS → STT API round trip, not a microphone E2E test.
- Moving the colour slider changes the panel's accent/background immediately; cancelling restores the saved palette. Temporary test voice selection was cleared afterward; the user's name/palette and saved key were retained.
- Default generated HTML and WASM were inspected for absence of the debug conversation composer. The optimized binary also excludes the debug-only voice smoke entry point; Enter did not open a text interface in the optimized native app.
- Optimized macOS app bundle built at approximately 17.34 MiB. Initial release builds reproduced an upstream Mach-O LINKEDIT alignment failure in stripped proc-macro libraries; explicit `strip = "none"` for release/build dependencies resolved the same build command.
- The optimized app launched using the saved palette. Renderer initialization now guards all drawing until persisted settings arrive; CSS also hides the canvas until that point. App data directory permissions are 0700.

## Important boundaries

- The user confirmed hearing a spoken reply after using the optimized app, verifying microphone → STT → Terra → TTS → speaker flow. Capture-device removal and other hardware failure modes have not been tested. The implementation uses explicit, bounded push-to-talk; it is not realtime/full-duplex streaming.
- Only ElevenLabs is currently registered. Other providers can implement the Rust provider interface; no additional provider is advertised as implemented.
- Desktop history reading is verified; seamless shared Desktop runtime ownership and live Desktop UI refresh are not.
- Memory retains explicit requests and performs deterministic source-linked synthesis/compaction. Automatic importance extraction from general interactions and LLM semantic distillation are not implemented.
- Forgetting excludes notes and dependent summaries from recall. It does not securely erase conversation history or underlying historical versions.
- App-server remains an experimental external runtime dependency. Host approval/elicitation and desktop-only tools unsupported by notdot are rejected.
- Local bundles are development artifacts; signing/notarization, public distribution, cross-platform packaging and full third-party notice generation are not release-validated.

## Remote CI

At implementation commit `569e989`, GitHub Actions run [40415491777](https://github.com/kabudu/notdot/actions/runs/40415491777) failed before executing any steps. Its check annotation states: “The job was not started because recent account payments have failed or your spending limit needs to be increased.” No runner logs exist because the job did not start. Local checks and the optimized bundle build passed; this does not substitute for remote CI. PR remains draft/unmerged pending account remediation and a successful run.

An additional pure Rust gesture regression verifies that a native window drag cannot activate recording when local coordinates return to their original position; primary clicks still activate, and secondary/control-clicks do not.

## Animation follow-up

The original amplitude-only motion was too subtle to establish an obvious thinking/speaking state. The renderer now uses an explicit mode mapping, a high-contrast rotating thinking halo, and radial waveform/body pulses measured from Web Audio during recording/playback. Native window visibility controls drawing; background suspension is disabled on macOS 14+ so the floating companion can keep updating without focus.

Two additional pure Rust tests cover waiting-stage/playback mode mapping and silence/known-waveform RMS. During a live native preview, the local diagnostic recorded `idle → thinking → speaking → idle`, advancing rendered frames, `reduced=false`, metering enabled during playback, and a measured peak speaking RMS of approximately 0.269. This is measured playback data, not a synthetic amplitude setting. A bounded local status file makes future visual-state failures diagnosable without storing speech content.

## Desktop lifecycle follow-up

At the owner's explicit request, GitHub Actions is now disabled at repository level, verified with the Actions permissions API. Workflow sources remain prepared; the release workflow also requires a public repository and an explicit public-release variable. The earlier billing failure above is historical evidence, not the current reason CI is absent. No release, tag, merge or visibility change was performed.

Local `scripts/ci-local.sh` passes for the reviewed working tree: core/provider/native tests, native and WASM Clippy (default/debug UI), gesture/visual regressions and the release-manifest regression. New coverage includes settings migration/startup preferences, interrupt requests carrying both identities, strict update consent, persisted update deferral, target-specific release URLs, and the real Tauri updater's HTTP/signature path. Synthetic signed content is accepted; tampering and version replay are rejected. No fixture is installed and this does not prove a full update/restart.

The Windows x64 current-user NSIS installer was transferred through the existing Windows App shared folder, SHA-256 checked on the laptop, installed and launched. Native UI showed the logomark in About, settings/switches, integrated voice picker and update controls. Testing exposed and corrected an extra console and oversized settings on the remote display. The corrected UI fits the available screen. `where codex` found no executable in the laptop's PATH; no Windows Terra/tool/voice round trip is claimed. No API key or account token was transferred from the Mac.

The macOS app and updater archive build locally with version-bound updater signatures; these remain development artifacts without Developer ID/notarization. Linux x64 compiles on Debian 12 and the Debian package builds. AppImage packaging succeeded after isolating a static-PIE packaging-launcher failure under Mac-hosted Linux emulation: the vendor plugin's extracted AppRun works where its AppImage launcher returns exec-format error. This is an emulation workaround for packaging tools, not a modification to notdot's runtime or signature checks. Final artifact inventory and the unclosed installed-platform gates are recorded in release-readiness.md.

Mac startup follow-up: a one-second native process sample identified `get_password` waiting in macOS Security on the main setup thread. The corrected launch uses a background metadata-only Keychain query and keeps secret decryption in the existing voice authorization path. Native launch, saved-key detection, preserved preferences and About were exercised after this fix. No credential ACL was changed and no credential value was read into test output.

The local release preview stages three real updater-signed artifacts (macOS arm64, Windows x64 and Linux x64), a complete latest.json and SHA-256 checksums without publishing. The Linux AppImage is approximately 110.89 MiB and Debian package 9.22 MiB. The final Linux package predates the macOS-only metadata-query adjustment; Linux credential behaviour is unchanged by that guarded code. A public release must rebuild from its exact tagged revision. The Mac later locked, so remaining live voice/installed-update trials were stopped; they are not claimed complete.

## Voice quota follow-up

The quota implementation passes `scripts/ci-local.sh`: 25 core/provider/native tests, three standalone interaction/visual tests, one Python release regression, formatting and native/WASM Clippy including debug UI. `cargo xwin check -p notdot --target x86_64-pc-windows-msvc --locked` also passes. Local HTTP fixtures exercise the real ElevenLabs client request boundaries with synthetic credentials: account usage, permission failure, payment/legacy quota errors, invalid key and rate limits. No paid calls are needed for these cases.

Ledger tests cover all thresholds, jumps, restart, account/cycle changes, stale/expired/unknown usage and unavailable accounting. Native monitor tests persist and reload delivery acknowledgements, preserve alerts with backoff after missing OS speech, reject old-credential results, and ensure successful transcription cannot clear a synthesis quota failure. Delivery tests inject the local speech callback; they do not establish OS speaker output. History is marked before speech to suppress crash/restart duplication; a crash during an announcement may lose that one spoken notice.

Live UI/account verification was attempted through the native app, but CUA returned “The Mac is locked and automatic unlock could not unlock it.” No lock bypass was attempted, no user's quota was depleted, and a live balance/speaker acceptance check remains open. Prior release-preview packages predate quota monitoring and must not be published as including it.

Provider sources: [user/account response](https://elevenlabs.io/docs/api-reference/user/get), [subscription fields](https://elevenlabs.io/docs/api-reference/user/subscription/get), [billing distinctions](https://elevenlabs.io/docs/overview/administration/billing), [typed errors](https://elevenlabs.io/docs/eleven-api/resources/errors). Account identity and subscription are obtained in one authenticated `/v1/user` request; the UI redacts identity while the local ledger uses it for per-account warning history.

The optimized macOS bundle and updater archive build successfully with local ad-hoc signing, and `codesign --verify --deep --strict target/release/bundle/macos/notdot.app` passes. The app is approximately 20.44 MiB; it has not been live-verified after this build because the Mac remains locked. No release preview was overwritten or published.

## Wake-phrase follow-up

Wake listening is opt-in and off for existing settings. The Rust worker owns one microphone and one keyword detector, downmixes callbacks through a bounded queue, and keeps all background audio local and ephemeral. Only PCM captured after detection is consumed through a one-shot native command into the existing STT client; the frontend receives the transcript and never receives raw wake audio. Settings/name/mute changes revoke the active capture epoch. Busy UI/voice states, playback, system announcements and missing frontend heartbeats pause listening; no-speech, duration/sample-count, queue-overflow and microphone-stall bounds prevent indefinite capture. Linux build prerequisites now include ALSA headers. Static sherpa-onnx and both selected model sets are included in the app; model checksums are verified during the build.

The original English BPE model failed the “Hey Ava” synthetic fixture. A newer pronunciation model plus its local dictionary handles dictionary-covered names; BPE remains a fallback for unknown spellings. Coarticulated “Hey Ava” needs a shared-vowel path, local onset-energy support and a short H→EY prefix check to reject the tested name-only false activation. These are bounded detector checks, not proof of universal acoustic accuracy.

Automated tests exercise actual bundled inference for “Hey Ava” at two UK synthetic speech rates, name-only “Ava”, unrelated speech and old-name rejection after a rename. A full worker-frame integration feeds the real keyword audio, observes capturing, supplies request samples and silence, takes the resulting WAV, verifies its exact samples exclude background/keyword audio, and rejects a second take. Separate cases cover timeout/no speech, continuous speech limits, settings migration/persistence, mute/pause/expiry/rename cancellation and keyword-directive rejection. No microphone or paid STT/TTS call is used by these tests.

`hey-ava-us.wav` is retained as a **known missed detection** for one US synthetic voice. It is not counted as a passing positive. Native microphone/accent, ambient-noise, echo and full wake→live STT→Terra→speaker acceptance remain unverified. CUA again reported the Mac locked; a request to unlock it was sent while independent validation continued. The passing fixtures justify an experimental feature, not a claim of reliable wake detection for all names or accents. Synthetic fixture provenance and redistribution limits are recorded alongside the files.

Final local wake validation: `scripts/ci-local.sh` passes 30 core/provider/native tests, three standalone gesture/visual tests, one Python release regression, formatting and native/WASM Clippy (default/debug UI). The detector also passes with 48 kHz simulated device input, exercising sherpa's actual resampling to 16 kHz. `cargo xwin check -p notdot --target x86_64-pc-windows-msvc --locked` passes with CPAL 0.18.2 and sherpa-onnx 2.0.0. These are compile/fixture checks, not a Windows microphone trial.

Linux x64 `cargo check -p notdot --locked` passes in the updated Debian 12 builder with ALSA headers, using the current source and static sherpa runtime. The first container attempt masked Cargo's executable directory with a cache mount; moving the cache to `/cargo` resolved that harness issue. The final incremental check also passes. This is Linux compilation, not an installed audio/desktop test.

The optimized macOS app/updater archive includes both checksum-pinned model sets and notices (approximately 58.7 MiB app, 22.8 MiB archive). A local Web Audio activation tone complements the listening animation without consuming voice-provider quota. The live activation tone, microphone permission and complete wake-to-reply path remain unverified while the Mac is locked. Existing release-preview packages remain untouched.

Final bundle verification: `codesign --verify --deep --strict` passes and all 17 packaged wake resources match their source bytes, including model weights, tokenisers and notices. GitHub Actions remains disabled, verified through the repository permissions API. No user preferences, credentials, release previews or public release state were changed.
