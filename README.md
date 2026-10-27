<p align="center"><img src="assets/logomark.svg" width="96" alt="notdot logomark"></p>

# notdot

**A voice companion. Just an orb. Your desktop, a little more personal.**

Talk to a small, expressive orb that listens, thinks and speaks. Built with Rust, Tauri and a Rust/WASM interface, notdot brings a persistent Codex conversation, optional local memory and your choice of colour to the desktop.

**Development preview.** No public binary release yet. macOS is the primary development platform; Windows and Linux builds still need hardware acceptance. Independent project, not affiliated with OpenAI or ElevenLabs.

## Meet your orb

- **Voice first:** click or press Space to talk; Escape cancels. Optional local name-based wake phrases are experimental.
- **Continuity:** one persistent companion conversation, with notdot as the writer. Desktop history viewing is available; live shared ownership is not established.
- **Memory you control:** optional source-linked learning, review, correction, forgetting and a complete local memory reset. This does not train model weights.
- **Make it yours:** name, live colour preview, searchable voices with previews, model and reasoning-effort settings.
- **Considerate by design:** separate learning and interruption permissions, quiet hours, quota alerts and reduced-motion support.

ElevenLabs is the first voice backend; the Rust interface supports adding others. GPT-6.1 Terra / Medium is the default, with available models and effort levels fetched from the installed runtime.

## See it in action

The [website source](docs/index.html) is ready for GitHub Pages. The real voice demo is **pending native recording and audio verification**; no simulated interaction is presented as a product demonstration. See the [recording and publication checklist](docs/demo.md).

## Run from source

You need macOS 14+, Rust 1.97+, an authenticated compatible Codex runtime, and an ElevenLabs account/key for voice. Services may require paid usage. The runtime is installed separately.

```sh
git clone https://github.com/kabudu/notdot.git
cd notdot
rustup target add wasm32-unknown-unknown
cargo install trunk --locked
cargo install tauri-cli --locked
cargo tauri dev
```

Open Settings with right-click or Cmd/Ctrl+, then configure your voice backend and choose a voice. Keys stay in the OS keychain. For platform prerequisites, bundles and configuration, read the [full guide](docs/guide.md).

## Where your data goes

Wake detection is local before activation. Activated audio goes to your voice provider; transcripts and recalled notes go to the model runtime. Memory and settings live in your application-data directory. Turning learning off stops ledger saving and recall without deleting history. Resetting local memory does not erase old Desktop chats or backups. See [security and privacy boundaries](SECURITY.md).

## Explore and contribute

[User & developer guide](docs/guide.md) · [Architecture and bridge investigation](docs/bridge-investigation.md) · [Validation evidence](docs/validation/2026-10-27-acceptance.md) · [Release readiness](docs/release-readiness.md) · [Changelog](CHANGELOG.md)

Bug reports and focused pull requests are welcome. Start with [Contributing](CONTRIBUTING.md), follow the [community conduct policy](CODE_OF_CONDUCT.md), and report vulnerabilities through [Security](SECURITY.md).

## Licence

Original notdot source and artwork: [Apache License 2.0](LICENSE). Dependencies retain their own licences; see [third-party notices](THIRD_PARTY_NOTICES.md). Product names belong to their respective owners.
