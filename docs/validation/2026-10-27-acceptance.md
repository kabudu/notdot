# Acceptance audit — 2026-10-27

Work in progress. Synthetic fixtures and isolated profiles only; do not run memory reset against the user's profile.

| Area | Evidence | Remaining acceptance |
| --- | --- | --- |
| Baseline | Local CI: 54 Rust tests plus three gesture/visual tests, one Python test, native/WASM Clippy | Rerun after fixes |
| Dependencies | Three ZIP extraction advisories traced to optional Windows updater ZIP support | Removed unused ZIP feature; audit vulnerability count is now zero; raw Windows EXE/raw Linux AppImage and macOS tar retained |
| Linux safety | RUSTSEC-2024-0429, GLib 0.18.5 string iterator; GTK3 pins old major | Vendored 0.18.5 with the two-line upstream fix; 57 optimized Linux variant tests pass; provenance hash checks cover all unchanged files |
| Voice | Earlier user hardware confirmation exists | New microphone → STT → selected runtime → TTS → speaker flow, including cancel/wake/error cases |
| Accessibility | Source review found no keyboard Settings shortcut, Enter activation gap, About-space microphone activation; theme Save contrast low at some hues | Keyboard/modal regressions and static ARIA/contrast tests pass; native VoiceOver/layout inspection remains pending |
| Performance | No current end-to-end measurements claimed | Record completed work, latency distributions, input size and environment |

Full hardware voice, VoiceOver and native visual checks require the Mac to be unlocked. Never call synthetic/API-only tests hardware E2E.

Security checkpoint: no advisory is suppressed. GLib is patched locally because GTK3 requires the 0.18 major. vendor/glib/NOTDOT-PATCH.md and UPSTREAM.json identify the original crate checksum, exact changed file and upstream fix. cargo audit still reports maintenance notices for other transitive dependencies; those are tracked separately from vulnerability findings.

## Live keychain regression found and fixed

A non-interactive synthetic voice probe blocked for over 71 seconds before reaching any provider call. A one-second native process sample located the wait in SecItemCopyMatching_osx → SecKeychainItemCopyContent → Security services. Query-level authentication-UI skipping was insufficient for this legacy keychain. The probe process was terminated without changing the user's running app or credential.

Silent macOS reads now enter a scoped legacy no-UI guard, preserving/restoring the prior process policy. All credential operations across vault instances share one operation mutex, preventing interactive operations from racing that process-wide setting. Cache-presence snapshots retain a separate short-lived lock. Native tests exercise policy restoration and nested pre-existing no-UI state without reading real secrets. The repeated synthetic voice probe returns an authorisation-required error promptly instead of blocking. Live provider/hardware voice acceptance remains blocked until the trusted test build is authorised locally; no authentication bypass, credential export or plaintext fallback is used.

## Reset safety follow-up

Reset intent creation now uses a create-new staging file, fsync and atomic rename instead of truncating a fixed path. Startup validates the exact regular-file intent before touching settings/data. New tests reject malformed requests without deleting memory or detaching the conversation, reject intent symlinks without overwriting their targets, and verify that memory/reminder cleanup only removes symlinks rather than following them outside the profile. Startup credential restoration also no longer performs a separate legacy metadata query outside the serialised no-UI path.

## Accessibility and UX corrections

- Enter/Space activate the orb; repeated keys and modifier combinations cannot inadvertently record. About/Settings/debug dialogs block global voice activation. Command/Control+comma opens Settings without losing an already-open form. Pure keyboard-action tests cover the modal and debug-mode matrix.
- The orb exposes button/pressed/busy semantics and concise live status; Settings/About have accessible names, and the voice picker associates its field label with the current value. Opening a memory editor now moves focus into it. Reduced-motion preference changes are observed while running.
- Save text had a minimum 3.007:1 contrast and failed 4.5:1 at 161 integer hues. Darkening the primary surface raises its minimum to 4.836:1 across all 361 hue positions. Selected-switch knob/track contrast is also tested against 3:1 for every hue. Source colour checks and ARIA reference tests are not a VoiceOver/browser audit or a WCAG conformance claim.

Reference: https://www.w3.org/WAI/WCAG22/Understanding/contrast-minimum.html. Native visual and screen-reader acceptance is blocked by the locked Mac.

## Isolated native acceptance preparation

A debug QA bundle/profile is prepared under com.kabudu.notdot.qa, separate from the production singleton, settings, ledger and conversation. Credentials are read-only: explicit authorisation can read an existing key, but saving/removal is rejected before OS access. QA startup mutations and live updater installation are blocked because those plugins otherwise share OS/global resources. The initial profile copies only non-secret voice/model preferences and disables automatic behaviour. scripts/prepare_qa.py refuses to overwrite a pre-existing profile. Native steps and remaining gates are in NATIVE-VOICE-ACCEPTANCE.md; the QA bundle is built but not launched while the Mac is locked.

## Capacity regression

A new backend regression failed before the fix: forgetting one of two records left total_records at two because soft-delete tombstones still consumed the 2,000-current-record budget. Forgetting now removes the selected current records and dependent summaries, freeing capacity for replacements. Existing forget/reopen/provenance tests continue to pass. Underlying store version/changefeed history can still remain until full Reset memory; this is not a forensic-erasure claim.
