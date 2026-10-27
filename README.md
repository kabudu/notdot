# notdot

A private, local desktop companion built with Rust, Tauri 2 and a Rust/WASM orb. The normal interface is just the orb. Conversation is voice-only; onboarding, preferences and memory inspection open separately.

## Run the development app

Requires macOS 14+, Rust 1.97+, `wasm32-unknown-unknown`, Trunk, Tauri CLI, and a supported installed Codex runtime signed into your ChatGPT account.

```sh
rustup target add wasm32-unknown-unknown
cargo install trunk --locked
cargo install tauri-cli --locked
cargo tauri dev
```

Build a local app bundle:

```sh
cargo tauri build --bundles app
```

Output: `target/release/bundle/macos/notdot.app`. This is an optimized, locally ad-hoc-signed development artifact, not a Developer ID signed/notarized release. Use `--debug` to build the larger diagnostic variant under `target/debug/bundle/macos/`. The installed runtime is discovered at the bundled ChatGPT macOS path, then `codex` on PATH. `NOTDOT_CODEX_BIN` can select an explicit runtime executable. The OpenAI runtime is a prerequisite, not redistributed inside notdot.

## Use

- Click the orb or press Space to start recording; click or press Space again to submit. Recordings stop after 60 seconds.
- Escape cancels recording, playback and an active agent turn.
- Drag the orb to move it. Right-click or use the tray's Settings item to customize it.
- The colour slider previews both the orb and settings theme immediately. Closing without saving restores the saved palette. Startup hides the orb until saved settings arrive, preventing a default-blue flash.
- Save a voice backend API key in Settings, load available voices, select one and preview it before saving. Search appears when there are more than five voices; longer lists use bounded pagination. The key is saved in the OS keychain and is never returned to the frontend. Audio goes to ElevenLabs only after explicit recording; transcripts go to the agent runtime. Background microphone listening is opt-in and local through the wake-phrase layer.
- Thinking has a clear rotating comet halo. Listening and playback use an audio-reactive radial waveform and body pulses, with a visible fallback when a meter is unavailable. Transcription and speech preparation use the waiting halo. Reduced-motion preference is respected. Rendering caps frame rate at 30 FPS and DPR at 2; drawing pauses when the native window is hidden/minimized. Webview background suspension is disabled so a visible, unfocused orb and its voice callbacks keep running.
- Enable **Learn useful context** for automatic, source-grounded learning, or explicit requests such as “Remember that I prefer jasmine tea.” Review, confirm, edit or forget notes in Settings. Disabling memory stops saving and recall; it preserves existing notes and conversation history.

## Agent ownership and Desktop

notdot keeps one long-lived `codex app-server --stdio` process, uses `gpt-6.1-terra` with Medium effort, and persists the conversation ID. The default thread is named `notdot · companion`. ChatGPT Desktop can read its stored history. Do not submit messages there: live shared ownership and live Desktop UI refresh have not been established.

Writer conflicts are shown as errors. notdot never removes locks or automatically archives a desktop-owned conversation. Reconnect is available from Settings/tray and explicitly interrupts an active turn before replacing the runtime. The app-server interface is experimental; a compatible installed runtime and account access are required.

The runtime starts with read-only filesystem access and untrusted approval policy. This build rejects host approval/elicitation requests it cannot implement; it cannot fulfill every desktop-only tool or permission flow. Connected read-only tools may work with the existing account configuration, but they must not be confused with a full desktop runtime attachment.

See [bridge investigation](docs/bridge-investigation.md) for the original experiments and limitations.

## Memory

Mnemara **0.6.0** is embedded as Rust library code with the sled backend. No separate database service is needed. Settings/thread identity use a small atomically saved JSON file. The voice-provider trait separates voice discovery, previews, STT and TTS from the UI; ElevenLabs is the first registered backend. To add another backend, implement `notdot_voice::VoiceBackend` and register its factory in `crates/notdot-voice`. Credentials are scoped per provider, and audio bytes include their MIME type. There is no arbitrary dynamic-library loading.

Memory data and configuration live under the platform application-data directory; on macOS that is `~/Library/Application Support/com.kabudu.notdot`.

The **Learn useful context** switch controls ledger saving and automatic recall. When enabled, a background Terra Medium pass extracts up to four durable preferences, goals, constraints or facts directly stated by the user after a successfully completed interaction. Explicit “Remember…” requests are retained at submission, even if conversation inference fails; cancelled turns are excluded from automatic learning. Each automatic note keeps an exact user quote, confidence and source conversation/turn. First observations are provisional for up to 30 days and excluded from recall and summaries. Another independent turn affirming the same keyed fact promotes it; replaying one turn does not. Explicit correction cues can replace inferred facts immediately. A contradictory passing remark remains provisional while the existing confirmed fact stays available. Confirmed corrections retract superseded versions and dependent summaries. Settings offers edit/confirm/forget with confidence, date, evidence and lineage; user-authored corrections take precedence over subsequent automatic inference. Editing and forgetting remain available with learning off.

The learner uses its own runtime process and ephemeral threads with shell, apps, plugins, MCP, browser and other action capabilities disabled for analysis. Structured results are validated before storage. It uses additional account inference, not model-weight training or psychological profiling. Model extraction can misinterpret a statement; notes remain observed data, never trusted instructions.

On each interaction, and during enabled idle maintenance every 30 minutes, the learner can distil existing facts into a shorter digest with explicit source links. A review considers up to 24 relevant/recent facts / 12,000 content bytes; recall remains bounded to six items / a 2,000-token budget. Only one learning job runs at once, with eight queued jobs. Overload skips interactions and reports the skipped count in Settings. Each model pass has a 90-second deadline; errors leave the conversation usable and appear in memory status. These bounds do not guarantee every interaction will become a note.

Turning the switch off prevents new ledger reads/writes by the companion and invalidates queued/in-flight learning results. It does **not** delete existing notes, the persistent chat, or notes already supplied in earlier conversation context. No retroactive processing of off-period chat history occurs when learning is re-enabled. Forgetting excludes a note and dependent summaries from active recall, while conversation history and store history/tombstones remain; this is not secure erasure. Ingestion stops at the 2,000-record limit. The review surface shows at most 100 current records.

### Behaviour and follow-ups

**Reactive** is the migration/default setting: respond only when addressed. **Suggestions** surfaces due reminders and inferred follow-ups as a small silent orb dot. **Proactive** announces eligible, individually permitted items through local OS speech while the voice pipeline is idle. Memory permission never grants interruption permission. Inferred follow-ups need **Allow spoken check-in** in Settings; explicit reminder requests grant permission for that reminder, but still require Proactive mode for automatic speech. No scheduler path calls model tools, sends messages, changes project files or grants external-action authority.

Say **“Remind me in ten minutes to take a break”**, using whole-number minutes, hours or days (one minute to thirty days). Unsupported date expressions explain the supported format instead of creating a guessed schedule. Say **“What are my reminders?”** or **“What are my suggestions?”** to hear up to five pending items, even in Reactive mode. Explicit reminders work independently of memory and do not require the Codex runtime to create them. Their local receipt lives in the reminder journal, rather than being appended to the model chat.

A separate journal discovers confirmed, source-linked goals/projects/commitments with semantic keys beginning `goal_`, `project_`, `commitment_` or `ongoing_`. Discovery runs every 30 seconds while learning and a suggestion mode are enabled. These are conservative offers to revisit recent context, not an unrestricted agent that guesses deadlines or continuously reasons over all history. Each inferred offer expires seven days after its source update and is removed when its source is forgotten/corrected or memory is disabled. Unchanged dismissed items do not reappear. Review allows permission, one-hour snooze and dismissal/completion. This is not a general calendar/date-language parser or automatic task-completion detector.

Quiet hours use the computer’s local time (default 22:00–08:00), with a configurable daily cap (default three, range one–ten) and minimum gap (default one hour, range fifteen minutes–one day). Due items wait until these conditions and voice-idle checks pass. The bounded journal holds at most 256 entries. Delivery is persisted before audio for at-most-once automatic attempts across restart: a crash or unavailable system voice can miss an announcement. Such items remain reviewable and can be snoozed to retry. Speech errors appear in Settings. Escape cancels a current spoken follow-up; changing to Reactive, dismissing an item or disabling its memory permission cancels the local speech process. This layer never silently enables provider overages or switches providers.

Mnemara gives us memory semantics we would otherwise implement over SQLite; no comparative speed or durability claim is made. [Mnemara](https://github.com/deliberium/mnemara) is MIT licensed and compiled into the app.

[QATQ](https://github.com/kabudu/qatq) is not used: it compresses exported tensors/KV caches, while the Terra bridge exposes conversation events and no KV-cache export. It does not solve semantic note distillation. Revisit it only for a local inference/tensor-archive requirement with representative benchmarks.

## Voice boundary

The initial voice path is bounded push-to-talk using browser MediaRecorder, ElevenLabs Scribe v2 transcription, Terra, and Eleven Flash v2.5 speech. It currently buffers each recording and speech response; realtime STT/TTS streaming and full-duplex interruption are not implemented. Starting a new recording stops playback and interrupts an active agent turn. Old transcription/speech work is invalidated on cancellation before delivery.

Live provider and microphone validation depends on locally configured credentials and microphone permission. Do not infer end-to-end voice success from compilation or the agent smoke tests.

## Validation

```sh
cargo fmt --all --check
cargo test -p notdot-core -p notdot-voice
cargo clippy -p notdot-core -p notdot-voice -p notdot --all-targets -- -D warnings
cargo clippy -p notdot-orb --target wasm32-unknown-unknown -- -D warnings
cargo clippy -p notdot-orb --target wasm32-unknown-unknown --features debug-ui -- -D warnings
cargo run -p notdot-core --example bridge_smoke
```

The smoke example uses real account inference and stores a separate test conversation ID in ignored `.local/rust-smoke/`. Native and WASM tests cover settings restart, memory restart/forget/derived-summary retraction, explicit retention intent, interleaved protocol events, writer errors and runtime exit.

A text composer exists only in the explicitly enabled frontend `debug-ui` feature for development. The default frontend and bundled HTML have no composer or conversation transcript. To use it in development, run `trunk serve --features debug-ui` in one terminal, then `cargo tauri dev --config '{"build":{"beforeDevCommand":""}}'` in another. Normal app builds do not enable it. The historical Python scripts remain investigation tools, not application dependencies.

The debug native binary also accepts `--voice-smoke`: it synthesizes a fixed test phrase with the saved voice, transcribes the generated audio, and verifies the returned phrase without using the microphone. This consumes a small amount of voice-provider usage. This diagnostic is compiled out of release builds.

The release profile explicitly disables symbol stripping to avoid Rust/LLVM's macOS 27 Mach-O proc-macro alignment issue ([upstream report](https://github.com/rust-lang/rust/issues/157750)). The optimized local app bundle was built successfully with this configuration.

For animation diagnostics, `visual-status.json` in the app-data directory contains the current visual mode, a bounded recent transition list and peak speaking RMS. It contains no transcript or audio samples. Reports are local only and stop while idle; actual drawing pauses when the native window is hidden/minimized. Background webview suspension is disabled on supported macOS versions so a visible, unfocused orb remains animated.

## Desktop lifecycle and releases

The companion uses its own headless Codex runtime; the desktop window need not be foreground. Work/Chat conversations are not a supported bridge. Startup options, the branded About screen and updater implementation are described in [release readiness](docs/release-readiness.md). Public releases and installed cross-platform update acceptance remain gated. GitHub Actions is disabled while this repository is private; run `scripts/ci-local.sh` locally.

### Voice usage alerts

Settings shows provider-reported usage, reset time, overage status and last check, with a manual refresh. ElevenLabs usage refreshes every 15 minutes, or after voice activity with a one-minute debounce. Cached results older than one hour or past their reset are marked stale. Unsupported accounting or missing API-key permissions shows **usage unavailable**.

At 80%, 90% and 100%, notdot speaks a short system-voice alert between turns and shows an amber orb ring; an actual credit failure shows a red ring. Warning history survives restart and is scoped to the provider account/workspace and billing reset. If usage jumps across thresholds, only the highest reached threshold is announced. Unknown reset dates suppress proactive threshold announcements because no reliable billing cycle can be identified. macOS uses `say`, Windows uses System.Speech, and Linux requires `spd-say`; missing local speech retains the visual warning and retries after 15 minutes.

The ElevenLabs subscription pool is **not a guaranteed combined STT/TTS balance**. Separate API/PAYG balances, API-key caps and remaining transcription minutes are unavailable from this report. Credit exhaustion, authentication, permission and temporary rate limits are distinguished. notdot never enables extra spending or changes providers automatically. Usage caching stores account/workspace identifiers and warning history locally, without API keys or speech content.

### Hands-free wake phrases (experimental)

In Settings → Hands-free voice, enable **Listen for my wake phrase**, save, then close Settings. The default phrase follows the companion name: **Hey Ava**, for example. Name-only activation is optional and more prone to false triggers. Wake names currently use English letters, spaces, apostrophes and hyphens; the companion can have another name when wake listening is off.

A small green dot on the orb means wake listening is armed. A short local tone and the listening animation signal activation. After that, speak your request; pause briefly after the wake phrase to avoid clipping the first word. Rust records only audio after activation, stops after 1.2 seconds of silence following speech, and uses the existing STT → Terra → TTS flow. Listening resumes after playback and a short cooldown. Eight seconds without a request discards capture; requests are bounded to 30 seconds and under 8.1 MB of PCM WAV. No background recording is written to disk or sent to a service.

The **Mute microphone** switch and tray **Mute / unmute microphone** action stop wake listening and microphone requests. A grey mark indicates mute. Push-to-talk remains available when unmuted. Listening pauses during Settings/About, voice processing, replies, previews, update/usage announcements and installation, or when the orb window is hidden. A three-second UI heartbeat timeout stops capture if the frontend stalls. Microphone errors latch until you save wake settings to retry.

Sherpa-onnx 2.0.0 is statically linked, with approximately 15 MiB of bundled models/tokenisers/notices; there is no runtime model download, access key or Python dependency. Dictionary-covered phrases use the local English pronunciation model; other English spellings use a BPE fallback. Build-time checksums pin the resources. Linux builds need `libasound2-dev` and runtime access to ALSA/PipeWire/PulseAudio routing.

Detection is experimental: two UK synthetic recordings pass, while the retained US synthetic recording misses. Some names, accents, noise and nearby speech may trigger accidentally or fail to trigger. These fixtures do not establish reliable room-scale detection; live hardware/accent acceptance remains open. See docs/VALIDATION.md and the bundled model NOTICE.

### Start at sign-in

In **Settings → Startup & updates**, enable **Start notdot when I sign in**, leave **Start immediately — desktop app not required** selected, and Save. Disable the switch and Save to remove automatic startup. This uses the OS login mechanism on macOS, Windows and Linux; a graphical companion starts after user sign-in rather than before login. The setting is off by default and preserves existing startup preferences. The optional **Wait until I open Codex / ChatGPT** behavior keeps the existing background watcher. **Open desktop app too** remains a separate macOS option; it is unnecessary for notdot’s runtime.

### Model and effort

**Settings → Intelligence** lists models reported by the installed, authenticated Codex runtime. It uses `model/list` with pagination, excludes hidden/non-text models, and derives effort options/descriptions from each model’s supported list. New/legacy settings default to **GPT-6.1 Terra / Medium**. A one-click default reset is available only if that pair is currently offered. Native selectors support keyboard navigation/type-to-select; selecting a model keeps a compatible effort or chooses the runtime’s suggested default. Saving resumes the existing companion history with the chosen pair; background learning follows the same selection.

The picker loads on opening Settings and refreshes every minute while open, with a manual Refresh button. Settings validates a changed selection against a fresh catalog; Save also rechecks a successfully loaded picker. Each new conversation/resume, turn and memory analysis checks availability before inference. Retired/hidden models and removed efforts are not selectable after refresh. A previously saved unavailable choice requires an explicit replacement; no automatic model substitution occurs. Catalog failures disable the selectors and preserve preferences. Other settings can still be saved while discovery is unavailable.

Availability is what the installed runtime/account reports, not the full ChatGPT web model menu. Runtime/server caching and changes between discovery and submission cannot be eliminated by a picker: a later server rejection is surfaced as an error. Keeping the Codex runtime updated remains necessary. The catalog is not persisted or replaced by a hard-coded fallback. Source: [official app-server model discovery](https://learn.chatgpt.com/docs/app-server#list-models-modellist).

### Keychain access without repeated launch prompts

notdot stores voice keys in the OS credential store and keeps an authorised copy only in native process memory for the session. Startup, normal voice requests, Settings auto-loading and automatic usage checks use silent access: on macOS a scoped login-keychain query skips items requiring authentication UI; Windows reads the current user credential set; Linux searches only already-unlocked matching Secret Service items without calling Unlock. Missing/locked/denied access is reported instead of opening an unsolicited password dialog. Opening Settings does not automatically load voices until a key is already authorised.

Use **Settings → Authorize saved key** when access is needed, or enter/save a new key. Only that explicit authorisation and explicit credential writes/removal may show OS authentication UI. On macOS choose **Always Allow** if you want macOS to remember trust for this app; **Allow Once** deliberately grants only temporary access. An unlocked, trusted installation can restore its key silently on later launches. Locked keychains, changed permissions and changed/ad-hoc development binaries can still require reauthorisation. Stable distribution signing is needed for stable app identity across updates; this patch does not bypass macOS trust or promise prompt-free access for every rebuild. [Apple: keychain access choices](https://support.apple.com/guide/keychain-access/if-youre-asked-for-access-to-your-keychain-kyca1243/mac), [Apple: changed trusted apps](https://support.apple.com/guide/keychain-access/if-a-trusted-app-asks-for-keychain-access-kyca1331/mac).

Successful reads are serialised and cached per provider, so STT, TTS, previews and usage checks reuse the same authorised session. Updating/removing a key updates/clears that cache only after the OS operation succeeds. Cache strings use zeroising storage; provider requests may hold their own in-flight copies. No cache is written to disk, and stored secrets are never returned to the webview. The original keychain service/account identity is preserved for existing keys; no ACL is widened and no keychain password is stored.

### Reset memory and start afresh

In **Settings → Memory → Reset memory…**, review the confirmation and choose **Reset memory and restart**. This deletes the entire local Mnemara ledger directory, including provisional notes, archived/deleted versions and summaries, and clears the local reminder/follow-up journal. The app restarts with no saved companion thread ID, creating a new conversation instead of reusing old model context. Name, palette, voice/API key, model/effort, startup, behaviour, learning permission and other settings are preserved. No real memory is reset just by opening the confirmation.

Previous conversations remain in Codex/Desktop history; notdot detaches from them rather than deleting external chats. Backups, OS snapshots and other copies are not erased, and this is not forensic disk sanitisation. Reset is irreversible within notdot. Stop or finish an active turn/voice operation before confirming. A durable reset intent is written before restart; startup completes deletion before opening the ledger or launching runtimes. If deletion or settings persistence fails, startup stops with the reset pending for retry, so old context cannot silently reload. No automatic processing of old conversations occurs after reset.
