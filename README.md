<p align="center"><img src="assets/logomark.svg" width="96" alt="notdot"></p>

# notdot

**A voice companion for your desktop.**

Talk to an animated orb that listens and replies aloud. notdot works with the ChatGPT Codex app and can remember useful details with your permission.

[Getting started](#getting-started) · [Privacy](#privacy-and-memory) · [Contributing](CONTRIBUTING.md) · [Apache 2.0](LICENSE)

## What you can do

- **Talk naturally.** Click once, speak, then pause to send. Press Escape to cancel. Experimental wake phrases let you start hands-free.
- **Keep a conversation going.** Continue the same chat after restarting notdot. You can view its history in Codex.
- **Choose what it remembers.** Turn learning on or off, review and edit saved memories, forget individual notes or reset them all.
- **Make it yours.** Choose its name, orb colour, voice and how much it can interrupt you.

The orb shows when notdot is listening, thinking and speaking. Settings opens when you right-click the orb or use Command/Control + comma.

## Getting started

notdot is in development. Public installers are not available yet; macOS, Windows and Linux testing is in progress.

To use it, you need:

1. Codex installed and signed into your ChatGPT account.
2. A supported speech service and your own API key. Deepgram can transcribe your speech; ElevenLabs can provide the spoken replies. Service charges may apply.
3. A microphone and speakers or headphones.

For now, [build from source](docs/guide.md). In notdot’s Settings, add your speech-service keys and choose a voice. The [Settings guide](docs/settings.md) explains the available options.

Use notdot to send messages in its chat. Keep the Codex app for viewing that chat’s history; sending from both apps at once is not supported. The Codex window does not need to stay in the foreground.

## Privacy and memory

Wake detection runs on your computer. Background listening is not sent to a speech service. After activation, your chosen services transcribe your request and generate the spoken reply. Codex receives your request and any relevant memories.

Saved memories stay on your computer. Turning learning off stops saving and using those notes, while keeping existing notes and chat history. Resetting memory does not delete saved Codex chats or backups.

Voice-service keys are stored in your operating system’s credential store. Read [Security](SECURITY.md) for details and how to report a vulnerability.

## Contributing

Bug reports, documentation improvements and focused pull requests are welcome. Start with [Contributing](CONTRIBUTING.md) for setup, tests and the pull-request process. Please follow the [Code of Conduct](CODE_OF_CONDUCT.md).

| Looking for | Start here |
| --- | --- |
| Everyday settings | [Settings guide](docs/settings.md) |
| Build and development instructions | [Developer guide](docs/guide.md) |
| Changes between versions | [Changelog](CHANGELOG.md) |
| Current testing and release status | [Release readiness](docs/release-readiness.md) |
| Website | [Website source](docs/index.html) |

## Licence

notdot’s original source code and artwork are licensed under the [Apache License 2.0](LICENSE). Bundled libraries and models keep their own licences; see [Third-party notices](THIRD_PARTY_NOTICES.md).

notdot is an independent project and is not affiliated with OpenAI, Deepgram or ElevenLabs.
