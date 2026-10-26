# Implementation contract

- Rust native backend, Tauri 2, Rust/WASM canvas renderer. Normal surface contains only a draggable orb; settings/onboarding and text entry are opt-in overlays.
- Single long-lived companion-owned app-server, explicit Terra Medium, durable thread identity, bounded messages/timeouts, no automatic archival/takeover, visible failures and cancellation.
- Rust voice provider with OS-keychain credential, explicit microphone capture, transcription and speech playback; no always-on capture by default. Provider errors must remain recoverable.
- Embedded Mnemara 0.6.0 for scoped, source-linked notes and bounded recall. Explicit memory controls, inspection/deletion and maintenance. Do not silently infer a permanent preference from an arbitrary conversation.
- First-run name, palette and voice preferences; later settings access. Saved atomically. No secret in settings or frontend state.
- Verify native and WASM builds, formatting, Clippy, lifecycle/error tests, real bridge smoke, memory reopen/deletion and native visual smoke. Report missing live provider credentials explicitly.
- Bundle a local development app, without claiming signing/notarization or production support. Existing installed Codex runtime is an explicit dependency.

Mnemara supplies memory semantics over embedded sled; this does not establish general performance/durability superiority to SQLite. It is packaged as Rust library code. QATQ is excluded because exported tensors/KV caches are not exposed by the Terra bridge; memory distillation is semantic selection/summarisation.


Additional user requirements incorporated:

- Production/default UI is voice-only. A conversation composer is available only through the explicit `debug-ui` WASM feature; native voice smoke diagnostic is debug-only.
- Speech providers are pluggable through the `notdot-voice` trait/factory boundary, including catalog, previews, STT and TTS.
- Voice selection uses discovered account voices with search above five entries and paginated loading, with preview before saving.
- Palette changes preview on both the orb and context panel and revert on cancellation.
- Startup must not paint a default-blue orb before persisted settings load. The renderer stays hidden and does not draw until settings initialize.

Current validation and remaining boundaries are recorded in VALIDATION.md. This file is an acceptance inventory, not a claim that a production release or every long-term memory capability is complete.
