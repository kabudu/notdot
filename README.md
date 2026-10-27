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
- Save a voice backend API key in Settings, load available voices, select one and preview it before saving. Search appears when there are more than five voices; longer lists use bounded pagination. The key is saved in the OS keychain and is never returned to the frontend. Audio goes to ElevenLabs only after explicit recording; transcripts go to the agent runtime. There is no always-on microphone.
- Thinking has a clear rotating comet halo. Listening and playback use an audio-reactive radial waveform and body pulses, with a visible fallback when a meter is unavailable. Transcription and speech preparation use the waiting halo. Reduced-motion preference is respected. Rendering caps frame rate at 30 FPS and DPR at 2; drawing pauses when the native window is hidden/minimized. Webview background suspension is disabled so a visible, unfocused orb and its voice callbacks keep running.
- Enable memory to retain explicit spoken requests such as “Remember that I prefer jasmine tea.” Review or forget notes in Settings. Disabling memory stops new note ingestion and retrieval; it does not delete saved notes.

## Agent ownership and Desktop

notdot keeps one long-lived `codex app-server --stdio` process, uses `gpt-6.1-terra` with Medium effort, and persists the conversation ID. The default thread is named `notdot · companion`. ChatGPT Desktop can read its stored history. Do not submit messages there: live shared ownership and live Desktop UI refresh have not been established.

Writer conflicts are shown as errors. notdot never removes locks or automatically archives a desktop-owned conversation. Reconnect is available from Settings/tray and explicitly interrupts an active turn before replacing the runtime. The app-server interface is experimental; a compatible installed runtime and account access are required.

The runtime starts with read-only filesystem access and untrusted approval policy. This build rejects host approval/elicitation requests it cannot implement; it cannot fulfill every desktop-only tool or permission flow. Connected read-only tools may work with the existing account configuration, but they must not be confused with a full desktop runtime attachment.

See [bridge investigation](docs/bridge-investigation.md) for the original experiments and limitations.

## Memory

Mnemara **0.6.0** is embedded as Rust library code with the sled backend. No separate database service is needed. Settings/thread identity use a small atomically saved JSON file. The voice-provider trait separates voice discovery, previews, STT and TTS from the UI; ElevenLabs is the first registered backend. To add another backend, implement `notdot_voice::VoiceBackend` and register its factory in `crates/notdot-voice`. Credentials are scoped per provider, and audio bytes include their MIME type. There is no arbitrary dynamic-library loading.

Memory data and configuration live under the platform application-data directory; on macOS that is `~/Library/Application Support/com.kabudu.notdot`.

Memory supports source-linked notes, bounded retrieval, deterministic provenance-preserving synthesis, duplicate compaction and forgetting. A maintenance worker reviews enabled memory every 30 minutes while no agent turn is active; Settings can run it immediately. Forgetting retracts dependent summaries. Notes remain in conversation history and the store's historical/tombstone records; forgetting excludes them from active recall, and is not a secure erasure operation. New note ingestion stops when the ledger has 2,000 records (maintenance may add derived summaries) and displays up to 100 notes in its review surface.

This is explicit-request memory, not automatic inference of a psychological profile. Mnemara's deterministic synthesis is not an LLM semantic-distillation pass. Automatic importance extraction and stronger long-term consolidation remain separate product work.

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
