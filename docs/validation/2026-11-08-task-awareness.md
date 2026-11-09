# Generic task awareness

Based on v0.5.0, on branch `codex/task-awareness`. This record separates real Codex and system-speech checks from synthetic service and UI checks.

## Behaviour

Execution and outcome are separate. Owned tasks can be working, need attention or have finished. Outcomes are reported success, failed, partial or unverified. A typed attention reason can request approval, clarification, sign-in or help with a blocker; it cannot grant permission or answer the request.

The target receives a bounded result-report instruction alongside the authorised user request. notdot accepts a final report only from the owned chat and turn, with the matching per-task token. Checks and receipts remain agent-reported claims. A finished turn is never independent proof of success. Failed checks override a success claim; missing checks and malformed reports remain unverified. Runtime failure and interruption override success claims.

## Checks performed

| Boundary | Evidence |
| --- | --- |
| Core policy and parsing | Core unit suite and existing bridge integration suite passed. Cases include empty, duplicate, malformed, oversized and foreign reports, contradictory success, missing checks, unfinished work, approval, clarification and sign-in. |
| Real bridge and process lifecycle, synthetic Codex peer | Seven result cases passed: draft success, failed tests, partial research, unverified booking, runtime failure overriding success, clarification and sign-in. Foreign/duplicate events, working state, cancellation and early unsupported approval were also exercised through consent and the actual bridge. |
| Real Codex, Terra Medium | Tool-free disposable chats produced the expected writing success, partial research, failed delivery and unverified booking outcomes. The example archives only the chats it creates. No real booking, delivery or provider action occurs. |
| Persistence and privacy | Native journal tests passed for restart, deduplication, durable acknowledgement, disabled speech and profile visibility. Existing trusted-profile tests remain in the native suite. Old journal entries without task metadata still load. |
| Production WASM UI with synthetic snapshots | Twelve task-cue checks passed in Chrome: working, attention, review and success cues, matching status text, foreground preemption, restoration, acknowledgement and accessible count updates without a cue change. Screenshot inspection confirmed the attention mark stays inside the dark core. |
| Existing simulated voice workflow | All 67 checks passed, including consent, mute, hidden window, Settings, cancellation, sentence playback, follow-ups and the echo guard. Audio, capture and provider adapters in this harness are synthetic. |
| Real macOS system speech | The opt-in native test successfully completed four system-speech processes for draft success, failed tests, partial research and unverified booking. No paid speech keys were accessed. This establishes the speech path completed, not independent human or acoustic verification. |
| Static checks | Native and WASM Clippy passed with warnings denied; formatting, website structure, local documentation links and orb input checks passed. |

## Bounds and failure handling

- Reports are parsed from at most 16 KiB of final text, with at most 8 KiB of JSON, eight checks and eight remaining items. No extra model request or polling service is introduced for classification.
- Four owned tasks, twenty recent in-memory completion records and bounded queue admission remain in place. One early attention notice per task adds at most four queue records beyond the 24-record admission threshold. The existing 256-entry, seven-day journal bounds apply.
- Automatic speech stays within 301 characters and uses whole-word truncation. Failed checks take priority over optimistic summaries. Details remain in the source chat.
- Quiet cues do not enable spoken interruptions. Requested announcements retain idle, mute, visibility, quiet-hours, spacing, cancellation and echo rules. Entry delivery is claimed before playback; a failed voice process does not trigger duplicate automatic speech.
- A successful cue expires after eight seconds. Review and attention indicators remain until dismissal or expiry. Voice acknowledgement clears saved reports, never a running task or an approval requirement.
- Foreground listening, thinking, speaking and error states take priority. Shapes and status text supplement colour; reduced motion keeps the task cues steady. The same Canvas overlay works with WebGL and fallback rendering.
- Revoked read permissions remove visibility. Private state uses the existing active-profile and private-announcement gate.

## Validation limits

This work does not establish live monitoring of chats owned by Codex Desktop. Tracking still requires notdot to remain open; restarting cannot resume observation of an already running turn. Agent-reported receipts are not independent verification of the outside world.

The browser harness is not a microphone-to-provider or hardware-speaker test. Windows and Linux physical audio and native desktop cue appearance were not retested for this feature locally. Hosted platform checks are recorded on the pull request. No existing saved keys or OS permissions were changed.

## Repeatable checks

- `cargo test -p notdot-core --locked`
- `cargo test -p notdot --bin notdot --locked -- --test-threads=1`
- `cargo test -p notdot-orb --lib --locked`
- `cargo clippy -p notdot-core -p notdot --all-targets --locked -- -D warnings`
- `cargo clippy -p notdot-orb --target wasm32-unknown-unknown --locked -- -D warnings`
- `cargo run -p notdot-core --example task_awareness_smoke -- /path/to/codex`
- Opt-in audible check: `cargo test -p notdot task_reports_play_through_system_speech_without_provider_keys -- --ignored`
- Build the existing conversation harness, then run its normal suite and `#tasks-only` suite. These do not access a microphone or paid provider.

## Integrated task lighting

The task cues now use the orb's own theme-coloured light instead of white symbols, black borders or separate badges. Working moves a broad current through the core. Attention uses a paired breath. Success expands a soft bloom. Review keeps a cluster low in the core. Reduced motion fixes each pattern in place; spoken and screen-reader updates retain the exact meaning. These are aggregate task cues, not a visual list of every running task.

The normal fluid is compressed behind the task light to keep the cue visible without drawing a separate backing. Deep blue receives a lighter tonal treatment so it remains legible. Foreground listening, thinking and speaking immediately take priority. New successful-result counts replay the bloom.

A debug-only acceptance export invokes the production shader and Canvas fallback at controlled phases. It measures the meaningful light regions against nearby core regions, including maximum speech-wave brightness and a white desktop. The former badge-specific absolute backing-luminance cap no longer applies; the required 3:1 relative contrast and opaque-core checks remain. Antialiased fringes are not treated as solid colours. These samples validate the selected cue regions, not every transition pixel or complete application WCAG conformance.

The complete matrix passed **27,648 samples** across 16 hues, 24 phases, two orb window sizes (320 and 480 pixels), normal/reduced motion and the WebGL/Canvas cases. Minimum measured contrast was **3.0315:1**, above the **3:1** graphical-indicator target. All sampled core backing regions remained opaque, so desktop colour cannot change those interior ratios. The WebGL cases used a white desktop during this run; the repeatable harness now also selects a dark desktop for its shader case. [Raw results](task-cue-contrast.json). The 12 production-WASM state checks also passed, including foreground priority, accessible count changes and cue removal.

All 29 orb unit tests and debug/release WASM Clippy passed. The light/dark desktop and blue/purple theme previews were inspected in Chrome. These are renderer checks, not new Windows/Linux hardware acceptance.

To repeat after a fresh debug Trunk build: `python3 scripts/prepare_task_cue_preview.py`, serve the dist directory locally, open `cue-contrast.html` and run the checks. `task-cues.html` provides the silent theme/background comparison. Task lighting uses bounded shader arithmetic and a fixed number of fallback gradients, without production pixel readbacks.
