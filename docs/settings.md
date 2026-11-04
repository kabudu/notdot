# Settings, in plain language

Right-click the orb or press **Cmd/Ctrl+,** to open Settings. Drag the header to move the window. Each main section opens and closes when you select its heading. Current choices stay visible in the headings. Changes take effect when you choose **Save**; **Close** leaves your saved choices as they were.

## Make it yours

**Name** is what you call your companion. **Orb colour** changes both the orb and Settings while you try it. Closing without saving puts the saved colour back.

## Intelligence

Open the single Intelligence card to choose a model, then its **thinking level**. Higher levels can take longer to answer. Choose **Done** to close the picker, then **Save** to apply the choice. The starting choice is GPT-6.1 Terra with Medium thinking.

The choices come from your installed Codex tool and account. notdot checks them again before saving and sending a request. If a choice disappears, it asks you to choose another instead of switching silently. The same choice is used for replies and background memory checks.

## Other Codex chats

Ask something like **“Find my Codex chat about the website and remind me what we decided.”** notdot can find saved local chats by title and read relevant messages. If several match, it can ask which one you mean.

Choose what it may do under **Other Codex chats**:

| Mode | What happens |
| --- | --- |
| **Read only** | Find chats and read messages. This is the default. |
| **Confirm before sending** | Read back the destination and instruction before each change. Say **“yes, send it”**, or **“yes, stop it”** for stopping a task. A bare “yes” does not approve it. |
| **Send when asked** | Act directly on an exact request such as **“Tell Website to make the button clearer”**, using the chat's saved name and your original words. Different or ambiguous wording is read back for confirmation. |

### Hear task results

Add **“and let me know when it’s done”** to a request, for example **“Tell Website to make the button clearer, and let me know when it’s done.”** Or turn on **Report task results** to receive reports for tasks notdot starts from then on. This does not change your chat-control permissions.

notdot reports when the exact Codex turn ends, using a short excerpt of its final reply. A finished reply does not independently prove that a deployment, background process or other external work succeeded. Failures, interruptions and lost connections have different messages; notdot never retries a send automatically.

Reports wait until you are idle, with Settings closed and the microphone unmuted. They respect your quiet hours. Requested results work even in Reactive mode and do not use the daily allowance for unsolicited check-ins; simultaneous reports are spaced at least five seconds apart. Reports use your computer’s voice, so they do not need another paid voice request.

The orb shows a pending follow-up dot. Review or dismiss reports in **Follow-ups**. If follow-up listening is enabled, you can answer after a report without another wake phrase. Listening starts only after speech ends and the echo guard has passed. Say **“That’s all”**, press Escape or stay silent to stop. Escape and opening Settings also cancel a spoken report.

Completed reports are saved locally for up to seven days. A claimed report is marked delivered before playback to avoid duplicate announcements after a crash; interrupted or failed audio remains available for review. Keep notdot open while tasks run: it cannot resume tracking a running task after restarting. Turning off Report task results affects future requests; dismiss already requested reports in Follow-ups to cancel them.

Each spoken request controls one chat action. With control enabled, notdot can send work to an available chat, add an instruction to a task it started, stop that task, and check its progress. A send acknowledgement means Codex accepted the request, not that the work finished. Ask notdot to check progress or read the chat for the answer.

Sending can start work using the target chat's existing model, working directory and permissions. notdot does not grant extra permissions. It cannot take over a chat held by the desktop app or another runtime, and it cannot steer or stop their active tasks. It explains that a handoff is needed instead of forcing one. Desktop-only tools and approval flows may still require the desktop app.

Approval is for one action only and expires after two minutes. Escape, muting, opening Settings, hiding notdot or a new request cancels pending approval. An action already accepted by Codex cannot be unsent. Changing to Read only prevents new actions; it does not stop work already running. Keep notdot open while tasks it started are running. Stopping cancels the current Codex turn; background processes may continue.

Selected chat messages go to the Codex model to answer your request. These controls do not provide access to chats that are not stored in the local Codex runtime. Empty, unsaved chats may not appear in its history index.

## Voice

With wake listening on, say your wake phrase. Orb clicks and Enter/Space do not start a separate recording. Drag still moves the orb, right-click opens Settings, and Escape cancels.

With wake listening off, click the orb once. A short chime tells you the microphone is ready; then speak and pause. notdot sends the request after 1.4 seconds of silence. A second click sends sooner; Escape cancels. Silence without speech is discarded after eight seconds.

**Keep the conversation going** is off by default. Turn it on in the wake and microphone settings, choose a follow-up window from **3 to 20 seconds** (normally **8**), then **Save**. After the complete spoken reply finishes, a short chime and two soft rings show that notdot is listening for another request. You do not need another click or wake phrase. If you start speaking within the window, you can finish your request normally; a pause sends it.

Say **“That’s all”** while it is listening, or press **Escape**, to end the exchange. It also stops on silence, mute, opening Settings or About, hiding the window, loss of voice-service access or an error. Ending an exchange returns to the usual wake or click mode. To interrupt a reply while it is speaking, use Escape; voice interruption is a separate planned feature.

Follow-up audio is sent to your chosen speech-recognition service and may count towards its usage. The microphone stays closed during notdot’s spoken reply and the short pause afterwards. Follow-up listening currently requires a streaming speech service.

**Speech recognition** normally uses your voice service. To try **Deepgram Flux · experimental**, select it, keep **Spoken language** set to **English**, enter its key in **Deepgram API key**, then choose **Save**. Leave the key blank to keep a saved one; use **Allow saved Deepgram key access** if macOS needs approval. Your selected ElevenLabs voice still handles replies. Deepgram live acceptance is pending; no accuracy or speed improvement has been established yet. It uses one streaming pass, with no premium hints or second decoding pass. Its usage is separate from the voice-usage bar. In QA builds, the Deepgram key has a separate keychain entry from the normal app.

**Spoken language** starts with English, which gives the service a language hint. Choose **Detect automatically** for other languages. This does not guarantee that every word will be recognised correctly.

A **voice service** turns replies into speech and, unless you choose separate recognition, turns recordings into words. With ElevenLabs, notdot starts sending your request after activation while you speak, so transcription can run sooner. Only final transcription results are sent to the model. If live transcription fails, notdot can retry the captured request. Escape cancels and prevents that retry; audio already sent may still count toward usage. Replies are prepared a sentence at a time so speech can start sooner. Escape stops playback and discards queued speech; requests already sent to the service may still count toward usage. ElevenLabs is currently the available service. Save its API key, choose **Load voices**, and pick a voice. Use the play icon to hear a sample. Search is built into the voice picker when the list is long.

An **API key** is a secret that lets notdot use your service account. It stays in your computer’s secure key store. macOS may ask you to allow access, especially after a development build changes. Do not put a key in an issue, screenshot or chat.

**Voice usage** shows the provider’s reported allowance: percentage used, credits left and reset date. The progress bar follows that percentage. Open **Details** for billing limits and the last successful check. An old balance is marked as the last known balance.

A key can allow speech but block account-usage checks. In ElevenLabs, the key needs **User → Access** to read the account information used here. You can keep voice working without balance monitoring. The displayed allowance does not tell you exactly how many recording minutes or replies remain. notdot warns at 80%, 90% and 100%, once at each level per billing cycle. It does not turn on extra spending.

If you use Windows through Windows App on a Mac, enable **Microphone** in the saved connection's **Devices & Audio** settings, then reconnect. Windows should list **Remote Audio** as a recording device. This forwards your Mac microphone to the Windows session.

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

If backup wake recognition fails on a phrase, the primary detector keeps listening and the backup can retry on the next phrase. Settings shows a short warning while that backup failure remains unresolved. Initial model-loading or microphone errors still need attention; notdot does not silently grant microphone permission.
