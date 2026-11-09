# Changelog

## [Unreleased]

## [0.7.0] - 2026-11-09

An easier first conversation, more careful memories and less background work.

- **Guided setup.** Connect Codex, follow official installation guidance, sign in and test a spoken conversation, with clear retry and cancellation.
- **Grounded memories.** New learned facts preserve your original wording, with bounded memory checks that skip simple acknowledgements.
- **Clearer check-ins.** Ask why a recent update was suggested, say “Later” or dismiss it by voice, with existing quiet hours and permissions respected.
- **Lower overhead.** Idle memory-analysis runtimes close after two minutes, task-status polling avoids unnecessary journal copies, and the orb draws fewer frames behind Settings.
- **No extra pass for empty speech.** An empty transcription no longer triggers a second batch request.
- **Easier troubleshooting.** Export a small report without keys, chats, memories or recordings.

## [0.6.1] - 2026-11-09

Chat permissions now take effect when saved, and task updates are available to screen readers.

- **Reliable chat permissions.** Changing access to other Codex chats refreshes the connection automatically, including when returning to Read only.
- **Accessible task updates.** The orb’s accessible label includes the current task status and updates when that status changes.

## [0.6.0] - 2026-11-09

Keep track of delegated work without opening every Codex chat.

- **Know what needs you.** Ask what is running or needs attention, with requested spoken updates and subtle, themed cues inside the orb.
- **Understand the result.** Reports distinguish success, failure, partial results and unverified work across coding and everyday tasks. Checks are reported by the task, not independently verified by notdot.
- **Stay in control.** Listening and speaking take priority over task cues. Dismiss saved updates without stopping work, with reliable status after cancellation and restart.

## [0.5.0] - 2026-11-08

A new fluid orb and clearer settings make notdot easier to see and use.

- **A fluid, luminous orb.** A dark core stays clear over other apps, with themed waves, a travelling thinking light, a smooth speech-led pulse and matching app icons.
- **Clear listening feedback.** An inner glow acknowledges your wake phrase, with smoother speech starts.
- **Clearer glass settings.** A subtle themed backdrop keeps labels and controls readable, with solid surfaces when reduced transparency or high contrast is preferred, reliable dragging and responsive Linux controls.

## [0.4.0] - 2026-11-07

Let the people you live with talk to notdot too, each with their own conversations and memories.

- **Recognise trusted voices.** Keep separate conversations and memories, with explicit sharing for other Codex chats. Voice matching stays on your computer and is not authentication.
- **Remove people safely.** Remove someone in Settings, or let them leave by voice with a one-time spoken code and a confirmation on screen. Their saved voice and private memories are deleted.
- **Catch a change of speaker.** If someone else takes over partway through a request, notdot asks one person to ask again instead of sending it.
- **Change settings by voice.** Hear proposed changes and approve before saving, or use a cleaner Settings panel with categories and search.
- **Hear task results.** Ask for a spoken report when delegated Codex work ends, with quiet hours, failure handling and optional follow-up listening.
- **Measure releases.** Define voice reliability and response-time targets, with checks that reject incomplete acceptance evidence.

## [0.3.0] - 2026-11-04

Work with your other local Codex chats by voice, with control over when notdot may act.

- **Find context.** Read relevant messages from other Codex chats.
- **Choose control.** Keep access read-only, require spoken confirmation, or allow clear requests to be sent directly.
- **Manage work.** Send instructions, add follow-ups and stop tasks that notdot started.
- **Recover wake listening.** Restart interrupted Windows wake helpers safely and show recognition problems in Settings.

Other-chat access is read-only by default. Chats already in use by the Codex desktop app need a handoff before notdot can control them.

## [0.2.0] - 2026-11-04

Keep a conversation going without repeating your wake phrase or clicking the orb.

- **Optional follow-ups.** Choose a listening window of 3–20 seconds after spoken replies, with two soft orb rings showing when you can answer.
- **Easy stopping.** Say “That’s all”, press Escape or stay silent to end the conversation. The microphone pauses while notdot speaks.
- **Clearer listening cues.** A short cue confirms the microphone is ready, helping avoid clipped opening words.
- **Better voice handling.** Preserve sentence endings on Linux, close empty follow-ups quietly and improve local wake responsiveness.

Follow-up listening is off by default. Enable **Keep the conversation going** under **Settings → Wake phrase and microphone**. Audio during that window goes to your selected speech-recognition service and may count towards usage.

## [0.1.0] - 2026-10-30

Talk to notdot through a desktop orb that listens and replies aloud using ChatGPT Codex.

- **Speak hands-free.** Use your chosen wake phrase, or click once and pause when you finish.
- **Choose a voice.** Search and preview voices, with separate choices for speech recognition and replies.
- **Control memory.** Review, edit, forget or reset saved notes, and choose whether notdot learns useful context.
- **Make it yours.** Set the name, orb colour, reminders and startup preferences.
- **Stay up to date.** Get update notices and choose when to install, with progress and automatic restart.

Download the universal Mac disk image for Apple Silicon or Intel, the Windows installer, or the Linux AppImage or Debian package. Windows installers do not have publisher signing. Linux corresponding sources and third-party terms accompany the downloads.

You need Codex signed into your account and your own speech-service keys. See the [setup guide](https://github.com/kabudu/notdot/blob/master/docs/guide.md) and [Settings guide](https://github.com/kabudu/notdot/blob/master/docs/settings.md).
