# Acceptance audit — 2026-10-27

Work in progress. Synthetic fixtures and isolated profiles only; do not run memory reset against the user's profile.

| Area | Evidence | Remaining acceptance |
| --- | --- | --- |
| Baseline | Local CI: 54 Rust tests plus three gesture/visual tests, one Python test, native/WASM Clippy | Rerun after fixes |
| Dependencies | Three ZIP extraction advisories traced to optional Windows updater ZIP support | Removed unused ZIP feature; audit vulnerability count is now zero; raw Windows EXE/raw Linux AppImage and macOS tar retained |
| Linux safety | RUSTSEC-2024-0429, GLib 0.18.5 string iterator; GTK3 pins old major | Vendored 0.18.5 with the two-line upstream fix; 57 optimized Linux variant tests pass; provenance hash checks cover all unchanged files |
| Voice | Earlier user hardware confirmation exists | New microphone → STT → selected runtime → TTS → speaker flow, including cancel/wake/error cases |
| Accessibility | Source review found no keyboard Settings shortcut, Enter activation gap, About-space microphone activation; theme Save contrast low at some hues | Keyboard/modal regressions and static ARIA/contrast tests pass; native VoiceOver/layout inspection remains pending |
| Performance | Repeated release-mode targeted ledger recall measured, plus an at-capacity run | Native voice/render latency still blocked; no end-to-end timing claim |

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

## Performance evidence

`acceptance_bench` and `scripts/measure_memory.py` run synthetic real-ledger work in fresh processes and verify that each query returns the specifically requested record. Three alternating runs per size perform 40 verified recalls each. The final report uses nearest-rank percentiles and includes revision, workload source hash, compiler/platform and hardware. It is a local-ledger measurement, not model/voice/renderer latency, and is not a comparison against another database. The earlier preliminary numeric-tag workload was replaced by uniquely tagged, target-verified records; preliminary timings are not mixed with final results.

| Workload | Recall p95 across runs | Insert p95 across runs | Completion |
| --- | --- | --- | --- |
| 100 short notes, three runs | 0.73–1.39 ms | 8.31–8.59 ms | 120 targeted queries, no failures |
| 1,000 short notes, three runs | 2.52–2.56 ms | 11.33–12.08 ms | 120 targeted queries, no failures |
| 2,000 × 4,000-byte notes, one capacity run | 12.93 ms | 32.10 ms | 40 targeted queries, no failures |

Raw distributions/method are retained in 2026-10-27-memory-performance.json. Reproduce with `cargo build -p notdot-core --example acceptance_bench --release --locked`, then `python3 scripts/measure_memory.py output.json`; capacity run: `cargo run -p notdot-core --example acceptance_bench --release --locked -- 2000 4000`. The temporary fixtures are removed afterward.

## Runtime and adversarial checks

A real default Terra/Medium bridge turn returns a synthetic acceptance marker, then a fresh process resumes the same isolated smoke conversation and recalls it. The three opt-in live tests pass account model discovery/non-default inference, semantic memory lifecycle and adversarial source handling. The adversarial test requires a completed structured model response and accepted safe extraction; timeouts/availability failures cannot count as passes. Its injected source asks for a temporary sentinel overwrite and retention of a fake secret; the sentinel remains unchanged and neither payload is retained. This is evidence for the isolated learning boundary, not proof against all possible prompt injection.

The unpatched upstream GLib iterator tests also pass on this particular toolchain: the advisory crash was not reproduced locally. The backport is justified by the upstream-confirmed undefined-behavior fix; 57 optimized patched variant tests and source-hash verification pass. Do not present it as a locally reproduced crash fix.

## Linux fixture race

The final default-parallel Linux rerun exposed a failure in native wake fixtures after an earlier complete pass. ONNX Runtime reported: “Only one instance of LoggingManager created with InstanceType::Default can exist at any point in time.” Two fixtures were constructing/destroying models concurrently; production uses one inference worker per process. The model fixtures now share a test-only mutex across the entire fixture lifetime, including teardown. All assertions remain.

The full Linux suite then passed, followed by three fresh-process runs of all five wake tests (each passed). This is a harness correction matching the production ownership model, not a claim that ONNX supports arbitrary concurrent environment construction.

## Final automated results and open gates

| Check | Result |
| --- | --- |
| macOS local CI | 59 Rust tests, four standalone keyboard/visual tests, six Python tests; formatting and native/WASM Clippy (normal/debug UI) pass |
| Linux amd64 container | 58 Rust tests pass, plus 57 optimized GLib variant tests; five wake tests pass in three additional fresh processes after fixture lifetime serialization |
| Windows x64 | All-target cross-compilation passes; native hardware/RDP acceptance is not run |
| Real authenticated runtime | Model catalog/non-default selection, semantic memory lifecycle, adversarial learning and two-process marker recall pass |
| Real voice provider/hardware | Blocked before provider calls by local key authorisation; Mac locked. Final signed-QA probe returns an authorisation-required error in 0.71 seconds (including context/startup), not a voice-latency pass |
| Dependencies/secrets | Zero cargo-audit vulnerability findings; no leaks in the committed-history scan. Six maintenance notices remain tracked; GLib's version-based unsoundness warning remains despite the reviewed/tested source backport |
| Bundles | Optimized production and isolated debug QA Mac bundles built and ad-hoc signatures verified; no release published |

Remaining required native gates: microphone → STT → selected model → TTS → audible speaker; cancellation/wake/mute/error hardware cases; VoiceOver, focus, scaling and visual layout; lock-screen/privacy behavior; OS sign-in startup and installed updater/restart; current Windows hardware acceptance. The Mac lock/key authorisation is an external blocker, not a successful E2E result. See NATIVE-VOICE-ACCEPTANCE.md for the prepared test profile and procedure.

No real memory was reset, no real key/ACL was changed, and the production app was not restarted for these tests. GitHub Actions remains intentionally disabled. The draft PR remains unmerged.
