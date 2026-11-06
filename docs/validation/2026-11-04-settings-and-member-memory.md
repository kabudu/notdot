# Settings and private member contexts

Validation for PR #26 on Apple Silicon macOS, Rust 1.97. The checks below use disposable profiles and synthetic data. They do not enrol the owner or alter installed app data or saved speech-service keys.

## Settings

The production WASM UI passed category, search, keyboard, narrow-window and About checks. The fixed header and actions remain visible while long content scrolls. Settings changes made by voice require a scoped readback and an exact confirmation. Tests reject bare yes, negation, a wrong-action approval, stale preferences and cancellation during asynchronous validation. The native save boundary checks both the original preferences and live approval authority before persistence.

## Private memory and chats

A native three-person lifecycle uses real Mnemara stores, not mocked memory. Owner and two members each save a different synthetic note. Each recalls only their own note, before and after closing and reopening the stores. A selected owner-chat read grant gives no write access, owner ledger access or access to the other member's private chat. Revocation rejects the old admission epoch while the member's own notes remain available.

The live Codex probes use Terra on Medium. Member and multi-person owner companions expose only the ACL-enforcing notdot tools. A synthetic file marker cannot be read through inherited shell, file, browser, app, plugin or MCP tools. Guessed chat IDs are denied before history is requested; discovery filters private titles and pagination tokens.

Members can receive a separate permission to add user context to a selected chat. This uses the supported raw-history append operation, starts no model turn and grants no device tools. The live probe verifies the forced user role, persisted text and unchanged permission history. Raw appended context is not a separate completed turn in the desktop history view; it becomes available when that chat continues. Owner task delegation retains its existing behaviour.

## Timing and cancellation boundaries

- Capture is identified locally before the speech provider opens. The buffered prefix is bounded; insufficient speech can be checked again up to the local limit. Revocation stops subsequent stream packets and disallows batch retry.
- A new person's Codex connection starts after transcription, so runtime startup cannot overflow the microphone's bounded channel.
- A capture retains an immutable person/epoch through model submission, memory confirmation and chat actions. A different speaker cannot answer an offered confirmation.
- Memory authority is checked after waiting for the mutation gate and before each write. Cancellation cannot undo a write that already committed.
- Task reports retain their origin in the private journal. Revoked reports are hidden; private speech requires recent recognised presence. Profile changes cancel old playback and queued sentences.
- Removing a person closes learning, runtime and data-store handles first. A held operation leaves a durable deletion marker; restart completes only explicitly marked removals.
- Enrolment cancellation and replacement are tested with inference blocked. Late results cannot restore the old session. Progress events carry the recording token, so stale events cannot advance a new enrolment.

## Evidence and limits

The full local release-profile suite, native/core/speaker tests, Clippy, WASM integration checks, accessibility and distribution-notice checks have been run. Detailed generated evidence is retained under `.local/task-followups/`; the live probes are reproducible from the core examples. See [speaker measurements](2026-11-04-speaker-identity.md) and [chat isolation](2026-11-04-profile-isolation.md).

Model-backed enrolment, admission, restart and recovery pass with synthetic speech files. Real household enrolment, microphone/accent accuracy and real-room replay remain unmeasured. Voice matching can be fooled and is not authentication. The OS account, Codex account, backups and direct access to local files remain outside notdot's profile boundary. Windows/Linux hardware timing is not established by the macOS measurements.
