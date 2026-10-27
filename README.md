<p align="center"><img src="assets/logomark.svg" width="96" alt="notdot logomark"></p>

# notdot

**A voice companion. Just an orb. Your desktop, a little more personal.**

Talk to a small, expressive orb that listens, thinks and speaks. Built with Rust, Tauri and a Rust/WASM interface, notdot brings a saved Codex conversation, optional local memory and your choice of colour to the desktop.

**Development preview.** No public binary release yet. macOS is the primary development platform; Windows and Linux builds still need hardware acceptance. Independent project, not affiliated with OpenAI or ElevenLabs.

## Meet your orb

- **Voice first:** click once or press Space, speak, then pause to send; Escape cancels. Optional local name-based wake phrases are experimental.
- **Continuity:** one saved chat. Send requests through notdot; use Desktop to read its saved history. Sending from both apps at once is not supported.
- **Memory you control:** optional memories linked to the chats they came from, review, correction, forgetting and a complete local memory reset. This changes what the AI knows about you; it does not retrain the model.
- **Make it yours:** name, live colour preview, searchable voices with previews, one model-and-thinking-level picker.
- **Considerate by design:** separate choices for remembering and interrupting, quiet hours, usage alerts and support for reduced motion.

ElevenLabs is the first voice service; the Rust interface supports adding others. GPT-6.1 Terra / Medium is the default, with available models and thinking levels checked with your installed Codex tool.

## See it in action

A new voice demo is being prepared. We’re waiting for a quiet recording session so you can hear both sides clearly.

[Website source](docs/index.html) · [Recording plan](docs/demo.md)

## Run from source

You need macOS 14+, Rust 1.97+, a compatible Codex tool signed into your account, and an ElevenLabs account/key for voice. Services may require paid usage. Install Codex separately.

```sh
git clone https://github.com/kabudu/notdot.git
cd notdot
rustup target add wasm32-unknown-unknown
cargo install trunk --locked
cargo install tauri-cli --locked
cargo tauri dev
```

Open Settings with right-click or Cmd/Ctrl+, then configure your voice service and choose a voice. Keys stay in the OS keychain. For everyday options, read the [Settings guide](docs/settings.md). Build and platform details are in the [developer guide](docs/guide.md).

## Where your data goes

Wake detection is local before activation. After you start a request, its recording goes to your voice service. The words from your request and relevant saved memories go to the model through Codex. Memory and settings stay in the app’s folder on your computer. Turning memory off stops saving and using those notes; it keeps your chat history. Resetting local memory does not erase old Desktop chats or backups. See [security and privacy boundaries](SECURITY.md).

## Explore and contribute

[User & developer guide](docs/guide.md) · [Architecture and bridge investigation](docs/bridge-investigation.md) · [Validation evidence](docs/validation/2026-10-27-acceptance.md) · [Release readiness](docs/release-readiness.md) · [Changelog](CHANGELOG.md)

Bug reports and focused pull requests are welcome. Start with [Contributing](CONTRIBUTING.md), follow the [community conduct policy](CODE_OF_CONDUCT.md), and report vulnerabilities through [Security](SECURITY.md).

## Licence

Original notdot source and artwork: [Apache License 2.0](LICENSE). Dependencies retain their own licences; see [third-party notices](THIRD_PARTY_NOTICES.md). Product names belong to their respective owners.
