# Acceptance audit — 2026-10-27

Work in progress. Synthetic fixtures and isolated profiles only; do not run memory reset against the user's profile.

| Area | Evidence | Remaining acceptance |
| --- | --- | --- |
| Baseline | Local CI: 54 Rust tests plus three gesture/visual tests, one Python test, native/WASM Clippy | Rerun after fixes |
| Dependencies | Three ZIP extraction advisories traced to optional Windows updater ZIP support | Disable unused ZIP format; retain raw Windows EXE/raw Linux AppImage and macOS tar |
| Linux safety | RUSTSEC-2024-0429, GLib 0.18.5 string iterator; GTK3 pins old major | Backport upstream fix and run optimized Linux regression |
| Voice | Earlier user hardware confirmation exists | New microphone → STT → selected runtime → TTS → speaker flow, including cancel/wake/error cases |
| Accessibility | Source review found no keyboard Settings shortcut, Enter activation gap, About-space microphone activation; theme Save contrast low at some hues | Fix, regress and inspect with native accessibility tools |
| Performance | No current end-to-end measurements claimed | Record completed work, latency distributions, input size and environment |

Full hardware voice, VoiceOver and native visual checks require the Mac to be unlocked. Never call synthetic/API-only tests hardware E2E.
