# Settings, in plain language

Right-click the orb or press **Cmd/Ctrl+,** to open Settings. Drag the header to move the window. Each main section opens and closes when you select its heading. Current choices stay visible in the headings. Changes take effect when you choose **Save**; **Close** leaves your saved choices as they were.

## Make it yours

**Name** is what you call your companion. **Orb colour** changes both the orb and Settings while you try it. Closing without saving puts the saved colour back.

## Intelligence

Open the single Intelligence card to choose a model, then its **thinking level**. Higher levels can take longer to answer. Choose **Done** to close the picker, then **Save** to apply the choice. The starting choice is GPT-6.1 Terra with Medium thinking.

The choices come from your installed Codex tool and account. notdot checks them again before saving and sending a request. If a choice disappears, it asks you to choose another instead of switching silently. The same choice is used for replies and background memory checks.

## Voice

With wake listening on, say your wake phrase. Orb clicks and Enter/Space do not start a separate recording. Drag still moves the orb, right-click opens Settings, and Escape cancels.

With wake listening off, click the orb once, speak, then pause. notdot sends the request after 1.4 seconds of silence. A second click sends sooner; Escape cancels. Silence without speech is discarded after eight seconds.

**Speech recognition** normally uses your voice service. To try **Deepgram Flux · experimental**, select it, keep **Spoken language** set to **English**, enter its key in **Deepgram API key**, then choose **Save**. Leave the key blank to keep a saved one; use **Allow saved Deepgram key access** if macOS needs approval. Your selected ElevenLabs voice still handles replies. Deepgram live acceptance is pending; no accuracy or speed improvement has been established yet. It uses one streaming pass, with no premium hints or second decoding pass. Its usage is separate from the voice-usage bar. In QA builds, the Deepgram key has a separate keychain entry from the normal app.

**Spoken language** starts with English, which gives the service a language hint. Choose **Detect automatically** for other languages. This does not guarantee that every word will be recognised correctly.

A **voice service** turns replies into speech and, unless you choose separate recognition, turns recordings into words. With ElevenLabs, notdot starts sending your request after activation while you speak, so transcription can run sooner. Only final transcription results are sent to the model. If live transcription fails, notdot can retry the captured request. Escape cancels and prevents that retry; audio already sent may still count toward usage. Replies are prepared a sentence at a time so speech can start sooner. Escape stops playback and discards queued speech; requests already sent to the service may still count toward usage. ElevenLabs is currently the available service. Save its API key, choose **Load voices**, and pick a voice. Use the play icon to hear a sample. Search is built into the voice picker when the list is long.

An **API key** is a secret that lets notdot use your service account. It stays in your computer’s secure key store. macOS may ask you to allow access, especially after a development build changes. Do not put a key in an issue, screenshot or chat.

**Voice usage** shows the provider’s reported allowance: percentage used, credits left and reset date. The progress bar follows that percentage. Open **Details** for billing limits and the last successful check. An old balance is marked as the last known balance.

A key can allow speech but block account-usage checks. In ElevenLabs, the key needs **User → Access** to read the account information used here. You can keep voice working without balance monitoring. The displayed allowance does not tell you exactly how many recording minutes or replies remain. notdot warns at 80%, 90% and 100%, once at each level per billing cycle. It does not turn on extra spending.

## Hands-free voice

Turn on **Listen for my wake phrase** to try “Hey” followed by your companion’s name. Wait for the orb to respond, then speak. A pause ends the request. Detection happens on your computer; only the request after activation goes to the voice service.

This feature is experimental and works in English. It can miss you or wake by mistake. A wake phrase is not proof of who is speaking. **Mute microphone** stops both wake listening and recording. Listening pauses while Settings is open or notdot speaks.

## Memory

Turn on **Remember useful details** to learn preferences, goals and important facts. Before saving a new fact from speech, notdot reads it back and listens for your answer. Say **“yes, remember that”** to save it, or **“no”** if it heard you wrongly. “Yes” alone, silence or Escape saves nothing. You can then repeat the corrected detail. This also applies to “Remember…” requests. Memory checks and spoken confirmations use your selected AI and voice services.

**Review memories** lets you read, confirm, correct and forget notes. **Tidy memories** reviews notes and makes shorter summaries. The review shows where a note came from and how sure the AI was; that confidence is an estimate, not a guarantee.

Declining a proposed memory keeps it out of the memory ledger; it does not erase the original transcript from chat history. Unconfirmed proposals expire and are not kept across restarts.

With memory off, notdot stops saving and using these separate memories. Existing notes stay on your computer. The current chat still has its own history, including anything already said or supplied to the model. Turning memory off does not erase that history.

**Reset memory** deletes all local memories, their older stored versions, reminders and suggestions, then starts a fresh companion chat. It keeps your settings and saved key. Old chats in Codex / ChatGPT, backups and other copies remain. The confirmation explains what will be removed; the reset cannot be undone.

A summary is checked separately from new facts. If it cannot be verified, notdot skips it and can still keep valid facts. The memory status tells you when a summary was skipped.

## Reminders and check-ins

Remembering something does not give notdot permission to interrupt you. Choose one behaviour:

| Choice | What happens |
| --- | --- |
| Only when I speak | notdot responds when you address it. |
| Show quiet suggestions | A small dot on the orb shows that a suggestion is waiting. |
| Speak when allowed | notdot can speak reminders and check-ins you have allowed. |

Try “Remind me in ten minutes to take a break.” Use whole-number minutes, hours or days, up to 30 days. Other date formats are not supported yet. Ask “What are my reminders?” to hear what is waiting.

Choose which suggested check-ins may be spoken. Quiet hours, the daily limit and the minimum gap can delay them. **Snooze** delays an item for an hour; **Dismiss** removes it from the list. These settings do not allow notdot to send messages, change files or take other outside actions without a separate request.

## Startup and updates

Choose **notdot → Check for Updates…** from the app menu, or open **Startup & updates** here. The same command is in the tray menu. On macOS, the notdot menu sits beside the Apple menu. Checks show their result in Settings; installing always needs your approval.

**Start when I sign in** opens notdot after you sign in. You can choose to wait until Codex / ChatGPT opens. notdot’s conversation service runs separately, so the desktop window does not have to stay in front. Opening the desktop app in the background is available on macOS.

When update checks are on, notdot checks every six hours. It asks before installing, shows progress and restarts afterward. Installed-update testing is still a release requirement; there is no public binary release yet.

## Connection and privacy

**Reconnect companion** restarts the conversation service. It interrupts any reply in progress. **Remove voice key** removes the saved service key.

Voice recordings go to the selected voice service. Words from your request and relevant saved notes go to the model through the Codex tool. Local memories are not separately encrypted by notdot; your computer’s account and backups matter. See [Security](../SECURITY.md) for the full boundaries.
