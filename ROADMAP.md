# Roadmap

These are the next additions planned for notdot. Priorities may change with user feedback; dates are not promised. See the [README](README.md) for the features available today.

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
- **Conversation continuity.** After you wake notdot and it asks a question, let you answer without another click or wake phrase. Offer an optional, bounded listening window for follow-up turns, with clear orb feedback, a timeout and an easy way to stop. Pause microphone capture while notdot speaks so it does not answer its own voice.
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
