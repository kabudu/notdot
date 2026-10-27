# Acceptance audit — 2026-10-27

Work in progress. Synthetic fixtures and isolated profiles only; do not run memory reset against the user's profile.

| Area | Evidence | Remaining acceptance |
| --- | --- | --- |
| Baseline | Local CI: 54 Rust tests plus three gesture/visual tests, one Python test, native/WASM Clippy | Rerun after fixes |
| Dependencies | Three ZIP extraction advisories traced to optional Windows updater ZIP support | Removed unused ZIP feature; audit vulnerability count is now zero; raw Windows EXE/raw Linux AppImage and macOS tar retained |
| Linux safety | RUSTSEC-2024-0429, GLib 0.18.5 string iterator; GTK3 pins old major | Vendored 0.18.5 with the two-line upstream fix; 57 optimized Linux variant tests pass; provenance hash checks cover all unchanged files |
| Voice | Earlier user hardware confirmation exists | New microphone → STT → selected runtime → TTS → speaker flow, including cancel/wake/error cases |
| Accessibility | Source review found no keyboard Settings shortcut, Enter activation gap, About-space microphone activation; theme Save contrast low at some hues | Fix, regress and inspect with native accessibility tools |
| Performance | No current end-to-end measurements claimed | Record completed work, latency distributions, input size and environment |

Full hardware voice, VoiceOver and native visual checks require the Mac to be unlocked. Never call synthetic/API-only tests hardware E2E.

Security checkpoint: no advisory is suppressed. GLib is patched locally because GTK3 requires the 0.18 major. vendor/glib/NOTDOT-PATCH.md and UPSTREAM.json identify the original crate checksum, exact changed file and upstream fix. cargo audit still reports maintenance notices for other transitive dependencies; those are tracked separately from vulnerability findings.

## Live keychain regression found and fixed

A non-interactive synthetic voice probe blocked for over 71 seconds before reaching any provider call. A one-second native process sample located the wait in SecItemCopyMatching_osx → SecKeychainItemCopyContent → Security services. Query-level authentication-UI skipping was insufficient for this legacy keychain. The probe process was terminated without changing the user's running app or credential.

Silent macOS reads now enter a scoped legacy no-UI guard, preserving/restoring the prior process policy. All credential operations across vault instances share one operation mutex, preventing interactive operations from racing that process-wide setting. Cache-presence snapshots retain a separate short-lived lock. Native tests exercise policy restoration and nested pre-existing no-UI state without reading real secrets. The repeated synthetic voice probe returns an authorisation-required error promptly instead of blocking. Live provider/hardware voice acceptance remains blocked until the trusted test build is authorised locally; no authentication bypass, credential export or plaintext fallback is used.
