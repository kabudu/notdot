# Security review — 27 October 2026

Scope: first-party Rust/WASM and Tauri code, runtime protocol and configuration, voice HTTP clients, credentials, memory and reminders, wake capture, updater and release scripts, frontend markup/CSP/capabilities, dependency advisories and repository secrets. This is a source and local integration review, not a third-party penetration test or certification.

## Findings and changes

| Finding | Impact | Remediation |
| --- | --- | --- |
| Inherited app/MCP automatic approvals | A permissive desktop configuration could allow remote write tools without notdot presenting a human decision. | Companion threads override default and per-tool approval modes to require approval for writes, retain stricter prompt modes, and use the human reviewer. Global Codex settings are unchanged. Unsupported host approval requests are rejected. |
| Late submission after cancellation | A submission returning after Escape could re-arm reply playback. | Check the capture generation after the awaited submission, before arming playback. |
| Speech program option parsing on macOS | Reminder text starting with an option such as `-f` could be interpreted by `say` as an option instead of literal speech. | End option parsing with `--`; regression checks the exact argument boundary. Linux already had a separator; Windows supplies text through an environment variable. |
| API-key header debug exposure | Future HTTP debug output could reveal a key header. | Store it as a sensitive HeaderValue; test redaction and invalid-header rejection. Secrets are not returned through IPC. |

Approval settings follow the [official Codex configuration reference](https://learn.chatgpt.com/docs/config-file/config-reference). Tool annotations and the installed runtime remain trusted enforcement inputs; notdot cannot secure a malicious provider or runtime. Read-only tool calls can still transmit queries and retrieve private data. There is no implemented external-write approval UI: calls needing it are denied rather than silently approved.

## Reviewed boundaries

- **Frontend:** bundled local content, restrictive CSP, one-window capability scope, no remote content permission or shell plugin. User/provider/model text is inserted through text nodes; the only innerHTML is constant debug-only markup. Native eval calls dispatch fixed event strings. No production text composer.
- **Runtime:** owned stdio child, read-only sandbox and untrusted approval policy, four-MiB frames, bounded request slots, write/request deadlines, rejected host requests, and explicit thread/turn identities for interruption. Configuration overrides are per thread and omit unrelated credentials.
- **Voice HTTP:** fixed HTTPS API endpoint, redirects disabled, bounded bodies/timeouts/concurrency, validated IDs and media types, no provider key attached to downloaded preview clips. Preview destinations are restricted to designated HTTPS hosts. Server error bodies do not pass through as arbitrary UI text.
- **Credentials:** OS credential store, serialized operations, native-only session cache, silent background reads, explicit interactive access, read-only QA mutations. No key was exported for review. Redaction is not a guarantee that every temporary HTTP buffer is zeroized.
- **Local storage:** app data is restricted to the owning Unix user. Windows relies on the profile directory ACL. Ledger and journals are bounded; writes are staged, reset intent is durable, and reset refuses malformed intents. Existing symlink/reset tests cover deletion scope. Same-user malware and forensic erasure are outside this boundary.
- **Memory:** opt-in, generation/revision invalidation, strict structured parsing, exact-source evidence, provisional notes, correction precedence and bounded ephemeral analysis with tools disabled. Memory instructions and keyword exclusions reduce risk but are not a general secret scanner or a guarantee against model mistakes.
- **Wake/capture:** background wake detection remains local, muted/renamed/cancelled captures are invalidated, leases expire and buffers are bounded. Manual capture now also stops after speech and silence, discards no-speech capture and retains a maximum duration. A wake phrase does not authenticate a speaker.
- **Proactivity/system speech:** scheduling grants no external tool authority. Quiet hours, delivery budgets and per-entry permission are separate from memory. Process arguments avoid shell interpolation.
- **Updater:** allowlisted version/platform GitHub paths, signature and version binding, bounded download and time budget, version-bound expiring spoken consent, serialized installation and no live install in QA. Tamper/replay fixtures exercise the actual verifier. Installed update/restart remains a release gate.
- **Supply chain/release:** locked dependencies, pinned Actions commits, least-privilege workflow tokens, gated draft release preparation, secret scanning and tracked-asset review. Actions, public releases and Pages remain disabled/unpublished.

## Validation and remaining gates

Current cargo-audit scan reports **zero vulnerability findings**, six transitive maintenance notices and the GLib unsoundness warning. The GLib source is locally backported; its exact provenance/hash regression and upstream variant checks are documented separately. Maintenance notices remain dependency risks, not a clean bill of health, and were not suppressed.

Local CI passed 62 Rust tests, eight standalone interaction/animation tests, ten Python checks and native/WASM Clippy. The authenticated adversarial memory test also passed against the real installed runtime: injected instructions did not change a temporary sentinel file or store a synthetic key. This is one bounded attack fixture, not a proof against all prompt injection. Native microphone/animation/cancellation retests are being completed separately. A source review cannot prove native lock-screen behavior, VoiceOver, Windows/Linux microphone handling, OS startup or installed updates. Those gates remain open before public binary release. The promotional recording remains on hold.
