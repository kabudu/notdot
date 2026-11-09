# Roadmap

Completed work and the next additions planned for notdot. Priorities may change with user feedback; dates are not promised. See the [README](README.md) for the features available today.

## Implemented

- **Translucent appearance.** A fluid dark-core orb and themed glass settings, with an inner listening glow, distinct conversation states, keyboard focus and reduced-motion support.

- **Trusted voices.** Keep separate conversations and memories for people you add. Share chosen Codex chats for reading, with messages allowed separately. Voice matching runs locally and is not authentication. [Settings guide](docs/settings.md#people-and-voices).

- **Change settings by voice.** Ask for a change, hear it read back and approve before it is saved. Settings also has focused categories and search. [Settings guide](docs/settings.md#change-settings-by-speaking).

- **Task awareness.** Follow work notdot starts with spoken updates and subtle orb cues. Ask what needs you or what is still running. Results distinguish reported success, failure, partial completion and unverified outcomes, with optional follow-up listening. [Settings guide](docs/settings.md#hear-task-results).

- **Use other Codex chats.** Find and read local chats, with optional controls to send work, add instructions and stop tasks notdot started. Choose Read only, Confirm before sending or Send when asked. Desktop-owned chats still require a handoff. [Settings guide](docs/settings.md).

- **Conversation continuity.** Turn on an optional 3–20 second listening window after spoken replies. An inner glow shows when you can answer; say “That’s all”, press Escape or stay silent to stop. The microphone pauses while notdot speaks. [Implementation and tests](https://github.com/kabudu/notdot/pull/22) · [Settings guide](docs/settings.md).

## More voice choices

Changing the speech service should keep your chosen Codex model, conversation and memories. Speech recognition and spoken replies should remain separate choices.

| Planned addition | What it means for you | Priority |
| --- | --- | --- |
| [OpenAI speech recognition](https://developers.openai.com/api/docs/guides/transcription) and [voices](https://developers.openai.com/api/docs/guides/text-to-speech) | Use OpenAI to recognise your speech, speak Codex's replies, or both. Follow the current supported APIs rather than building on retiring ones. | Next |
| [SpaceXAI / Grok Voice](https://docs.x.ai/developers/model-capabilities/audio/voice) | Use Grok speech recognition and voices through the xAI voice APIs, while keeping Codex responsible for the answer. | Next |
| Consistent setup across providers | Save a key securely, choose and preview a voice, and understand which service handles each part of the conversation. | Alongside new providers |
| Clearer usage information | Show separate recognition and speech usage where the provider supplies it. Let you choose any fallback before switching services or spending more. | Next |
| Local speech options | Explore recognising speech and speaking replies on your computer. Codex still handles the conversation; this is not a promise of a fully offline assistant. | Exploring |

A new backend needs streaming where supported, reliable cancellation, clear errors and platform tests. A service that cannot report usage should say so rather than display an invented balance.

## More natural conversations

- **Interrupt by speaking.** Stop a reply with your voice, without the assistant reacting to its own speaker output.
- **More languages.** Expand recognition, wake phrases and spoken replies together, with clear choices for the languages each service supports.
- **Clearer names and numbers.** Improve recognition of names, dates and similar-sounding words, with tests in different rooms and on different microphones.

## Memory, notes and reminders

- **Voice notes.** Dictate a titled note, list or idea, then find, hear, correct or delete it by voice. Allow export and keep these notes separate from learned preferences and facts.
- **Correct memories by speaking.** Make it easier to explain what changed, replace an old fact and confirm exactly what will be kept.
- **Understand suggestions.** Show which remembered facts led to a suggestion or check-in and make them easy to correct.
- **Richer reminders.** Support calendar dates, recurring reminders and a choice of snooze durations.
- **Private backup and restore.** Offer an optional encrypted export of memories and settings, with clear control over what is included.

## Connected tools and desktop support

- **Approve actions by voice.** Explain a requested tool action and ask for specific permission before changing files, sending messages or changing account data. An approval must apply to that action only.
- **Start the desktop app on Windows and Linux.** Extend the background Codex app launch option already available on macOS.
- **More accessible controls.** Add orb size, contrast and motion choices, and improve screen-reader coverage.
- **More ready-to-install packages.** Evaluate Linux ARM64 packages and other supported hardware using native installation, audio and update tests.

## Suggest an addition

[Open an issue](https://github.com/kabudu/notdot/issues/new) describing what you want to do and what gets in your way. New integrations should preserve the voice-first orb, local wake listening, clear memory controls and permission before taking outside actions.

See the [OpenAI voice assessment](docs/research/openai-voice.md) for speech services and the current distinction between API billing and subscription-funded desktop voice.
