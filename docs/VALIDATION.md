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
