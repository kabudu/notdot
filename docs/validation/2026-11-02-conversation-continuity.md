# Conversation continuity: implementation and acceptance

Status: feature branch `codex/conversation-continuity`; not released. Unattended Mac voice checks ran on 3 November after the owner approved saved-key access. The browser integration checks use synthetic services and audio; they are described separately below.

## Behaviour

- Off by default; existing settings load with it off and an eight-second window.
- Settings → wake and microphone → **Keep the conversation going**, with a 3–20 second slider.
- Only a completed spoken reply can start a window. The whole sentence queue must finish; gaps between streamed sentences do not count as the end of the reply.
- A 600 ms guard after playback precedes automatic capture. This guards sequencing; its adequacy for real devices and room echo still needs acoustic testing.
- Two soft rings and an accessible status distinguish follow-up listening. The existing speech detector ends a spoken request after a pause. Starting speech before the deadline does not truncate it at the window boundary.
- Whole-utterance **“That’s all”**, including a straight or curly apostrophe, ends follow-up listening before sending a message to the model. Longer requests containing those words remain ordinary messages. Memory/update consent keeps its own routing.
- Silence, Escape, mute, hidden windows, settings/about dialogs, errors and loss of voice-service readiness close the window without reopening it automatically. Scoped capture tokens prevent late cancellation from affecting a newer recording.
- Automatic capture uses the existing bounded native streaming path. Audio during the opt-in window reaches the selected recognition service and may count towards usage. Non-streaming backends fail closed rather than silently taking a different recording path.

## Automated validation

- `scripts/ci-local.sh`: native core, voice, wake-runtime and app tests; the orb’s pure Rust logic tests; formatting; native and WASM Clippy (including debug UI); existing Python accessibility, security and release-tool checks.
- Settings tests: backwards compatibility, saved on/off preference and duration, and rejection of invalid windows without overwriting saved values.
- Native capture tests: silence boundaries at 3/8/20 seconds at 16 kHz and 48 kHz; speech continuing beyond the initial window; normal silence endpoint; cancellation identity and mute rejection. These exercise captured samples, not a physical input device.
- Pure Rust scheduling tests: opt-in, completed playback, the guard interval, one automatic opening, permission withdrawal and exact stop phrases.
- Browser integration harness: actual built production WASM interface with explicitly synthetic Tauri IPC, capture, STT, model and audio adapters. It covers click- and wake-initiated exchanges, repeated turns, stop routing, no reopening after stop/timeout, Escape, mute, hiding and settings, in-flight activation cancellation, settings-control persistence, and capture exclusion during playback and inter-sentence gaps. All 14 browser scenarios passed, including provider-confirmed empty speech returning to idle without a model message or Settings dialog. This is integration simulation, not real microphone-to-model-to-speaker evidence.

Reproduce the silent browser checks with:

```sh
trunk build --dist .local/continuity-ui
python3.12 scripts/prepare_conversation_harness.py
python3.12 -m http.server 8095 --bind 127.0.0.1 --directory .local/continuity-ui
```

Open `http://localhost:8095/` and choose **Run silent integration checks**. The harness installs virtual audio adapters before loading WASM and rejects browser microphone requests. It uses only staged test phrases, no keys and no external service requests. Rebuild before preparing it again. Test adapters are not included in the app bundle.

## Native Mac voice evidence — 3 November

The running QA bundle was built from `75e3365`, with the production WASM interface, in a disposable profile. Memory, reminders and automatic updates were disabled. Existing OS credentials were used inside the app, with no key export. Generated test speech played through the Mac speaker into its physical microphone; Deepgram Flux transcribed it, Terra remained on Medium, and ElevenLabs Bella played the reply. This is acoustic integration evidence for these staged phrases, not human hearing or broad accent/recognition acceptance. No demo recorder ran.

| Check | Observed result |
| --- | --- |
| Wake-initiated continuity | Initial arithmetic question, two automatic follow-ups and spoken stop completed. The disposable Codex chat contains the three expected questions and answers **12**, **7**, **18**, with no stop phrase or self-generated model turn. |
| Spoken stop | “That’s all” ended the automatic capture and returned to wake listening. No further activation was observed during the bounded settling interval. |
| Click-initiated continuity | One click started capture. A follow-up began about 6.2 seconds into the 8-second window and continued beyond that deadline; the full question reached the model, which answered **14**. |
| Empty initial window | In the clean timeout attempt, capture ended about 8.1 seconds after opening, then wake listening resumed. No new model turn was added. |
| Embedded stop words | “In one short sentence, explain the phrase that is all” reached the model and received an ordinary spoken answer. |
| Escape during opening | The real follow-up label appeared, Escape cancelled activation, and no automatic reopening followed. |
| Escape during active capture | Native diagnostics show capture opening, then cancellation about one second later and return to idle, with no extra model message or reopening during nine seconds of observation. |
| Reply/capture separation | Observed captures begin after complete playback, with roughly 0.8–1.0 seconds between the reported playback end and native capture start in these runs. No model received notdot’s own reply. A later short automatic capture did produce a no-update STT timeout; see below. |

The first near-deadline test driver waited for `paused` at the end, but click mode correctly reports `disabled`. Its overall driver timeout was a harness terminal-state mistake: the native timeline and the independently inspected chat establish the completed question, reply and empty-window closure. Do not count that driver status as a clean automated pass.

### Defects found and changes awaiting rebuilt-native acceptance

- Native visual diagnostics rejected the new `followup` mode, although the actual orb displayed it. The whitelist now accepts every serialised UI mode, covered by a cross-crate protocol check.
- After the longer embedded-phrase answer, an automatic capture was submitted with no Deepgram speech updates and failed finalisation. No unintended text reached the model. The old adapter ignored all `Warning` events. [Flux documents](https://developers.deepgram.com/docs/flux/force-end-turn) a specific `FORCE_END_TURN_NO_ACTIVE_TURN` warning in place of a final transcript when there is no active turn. A local WebSocket regression reproduced the old timeout for this response; it now returns a typed no-speech outcome, closes the UI window and never retries a billable batch pass. Unknown warnings, missing responses, and a no-active-turn warning after observed speech remain failures. The live server event was not retained, so this protocol defect is established independently; it is not proof of the exact server response in that acoustic attempt.

The new no-speech path passed the provider WebSocket tests, native session tests, the full local validation suite and all 14 production-WASM browser scenarios. The running QA process was intentionally kept alive to retain authorised key access while the owner was away; it does **not** contain these final native changes. Repeat physical silence/echo acceptance on the rebuilt bundle before merging. Windows/Linux physical continuity checks have not yet run on this branch.

## CI wake deadline follow-up

`09ed078` completed full CI successfully, including the unsigned Windows installer. At `d39d083`, Windows and both Mac architectures passed, but Linux's first fallback corpus test exceeded the unchanged three-second recognition deadline. Later model tests failed because that panic poisoned the shared fixture mutex; they are not independent failures. Earlier Windows observations also showed a slow first helper inference followed by faster requests in the same process. This establishes a first-use cost worth preparing for, but does not establish that every intermittent deadline miss has the same cause.

The generic decoder and optional helper now run one bounded synthetic-silence inference before reporting ready. Preparation has an eight-second compute budget; initial helper readiness is capped at ten seconds including loading. Real wake utterances and helper restarts inside an utterance retain their three-second deadlines, strict address/prefix guards and cancellation behaviour. No microphone is open during preparation. The native worker rechecks mute, revision, epoch and the live permission lease immediately before opening it, because those may have changed during setup. A preparation failure keeps wake listening unavailable rather than pretending the decoder is ready. Generic fallback after a helper failure retains its existing per-request path; no retry is added to the failed utterance.

The full local suite passed with this change. Full platform CI at `1cdc5ea` passed: [run 41637718985](https://github.com/kabudu/notdot/actions/runs/41637718985), including the Windows NSIS installer and packaging checks. Rebuilt-native acceptance remains required. An initial direct shell launch of the Linux VM misleadingly reported missing microphone permission; the later normal app launch succeeded without a new permission prompt. No permission was weakened or prompt approved automatically.

## Further unattended checks — 3 November

The authorised Mac QA process remained alive; neither saved key was exported or replaced. A three-sentence reply (“Take a short walk. Stretch your shoulders. Drink a glass of water.”) played through three observed speaking intervals. Native recording stayed closed through all clips and their gaps, opened about 1.1 seconds after playback, then closed its silent follow-up after about 8.4 seconds. Twenty seconds of observation after opening found no additional model turn. This strengthens the acoustic evidence for `75e3365`; it does not substitute for testing the rebuilt final native code.

The staged final bundle's existing non-interactive voice smoke check returned a saved-key authorisation error without opening authentication UI. The debug STT evaluator had still requested interactive access; it now uses the same non-interactive credential path. Its isolated rebuilt process also returned a saved-key authorisation error without requesting a prompt. Therefore physical final-build voice acceptance cannot be completed from those new processes while the owner is away. The old app's in-memory key cache is intentionally preserved.

Windows App reported a lost connection to the test laptop. A bounded connection attempt to its configured RDP endpoint timed out. No Windows voice result is claimed for this session.

The native Linux VM starts successfully through the already authorised `notdot Linux VM` app, rather than invoking its executable from a shell. Its Ubuntu 24.04 ARM64 guest had booted kernel `6.8.0-147-generic` while only the older `6.8.0-142` extra-module package was installed. The VirtIO sound device was present, but `modprobe virtio_snd` failed because the current kernel's module was absent. Installing the matching official Ubuntu extra-modules package restored the driver and exposed separate stereo output and microphone routes. Installing the `linux-image-generic` metapackage keeps these modules aligned with subsequent kernel updates. This is a test-VM repair, not a notdot application change or a general Linux limitation.

A metadata-only Secret Service check reports the Linux default key collection as locked. No secret was read and no Unlock or Prompt operation was requested. Current-source offline capture/model tests can proceed; cloud-backed Linux STT/TTS acceptance remains unavailable until the existing key collection can be read normally.

### Linux native and final silence/echo results

The current production Linux code (`1cdc5ea`; `34d4a54` only changes the debug evaluator) was built for ARM64 and run on the Ubuntu 24.04 VM. The offline native suite passed **63 tests**, with **12 intentionally ignored**. A separate physical microphone diagnostic observed 424 quiet frames without a false wake. Playing the wake fixture and then a spoken fixture through the Mac speaker into the forwarded VM microphone produced exactly one activation, 36 audio packets and one completed voiced capture, followed by clean permission-lease shutdown. No cloud transcription or model reply was involved in that Linux diagnostic.

A separate guest-output/forwarded-input route test captured only in RAM: output and recording completed successfully, and the playback interval had about 3.4 times the baseline RMS. The production Linux GUI also launched in an isolated data directory without a credential prompt. Its follow-up switch and duration saved as enabled/12 seconds; the original Linux profile was untouched. The owned test app was closed afterward. These results establish local audio routing, capture and native settings behaviour, not full cloud-backed voice acceptance.

On the existing authorised Mac process, a reply containing “Hey, Ava.” completed, its follow-up window closed silently, and no extra model turn or self-activation appeared during 20 seconds of observation. One earlier attempt using the original wake fixture missed activation; the UK fixture activated successfully. Keep that miss in the evidence rather than claiming perfect reliability. The running process remains `75e3365`.

At `34d4a54`, hosted Intel Mac CI failed on the sixth framed fallback corpus decode (about 3.15 seconds against the unchanged three-second limit). Ten subsequent failures were shared fixture-lock poisoning from that first panic. This is a remaining intermittent decode-performance failure, despite the preceding full green run at `1cdc5ea`; first-use preparation did not eliminate it. No deadline, recognition guard or required check was relaxed. Current-head CI must be green and the timing failure understood before acceptance.

## Signed Mac and native Windows/Linux checks — 3 November evening

The owner approved both saved-key requests for the Developer ID-signed final Mac QA bundle. Both services then reported ready. No keys were exported, replaced or entered into tools. The production profile was untouched. This bundle contains `1cdc5ea` production code; the later debug-evaluator and evidence commits do not change its voice behaviour.

The final Mac build completed a streamed three-sentence reply with recording closed through every sentence and gap. Its silent follow-up closed after approximately 8.4 seconds, with no extra model message during the bounded observation. A UK wake fixture activated the companion, followed by three connected questions yielding 12, 7 and 18; “That’s all” ended the exchange without a model request. A reply containing “Hey, Ava” did not activate another turn. Speech beginning 6.2 seconds into the eight-second follow-up was captured to completion beyond that deadline. These tests use generated speech through the physical speaker/microphone route; they are not a new human listening assessment.

Current-source Windows acoustic checks also completed one-click arithmetic and an automatic follow-up (12 then 18), spoken stop, wake activation, a quiet echo window, and a question beginning near the deadline that answered 14. Escape during initial capture and mute-before-activation produced no model message. The original Windows profile was restored and the test app/observers stopped. One other quiet interval contained unexpected background speech and is excluded from silence acceptance. An echo request was not transcribed exactly; this remains an accuracy observation, not a reason to label all Windows speech reliable. In-flight follow-up cancellation still needs a physical Windows check.

The Linux Secret Service collection was unlocked through its normal service operation without a password prompt, and both saved voice services became ready. The isolated Ubuntu 24.04 ARM64 app completed native microphone → Deepgram → Terra Medium → ElevenLabs turns and spoken stop. The acoustic driver initially started fixtures when the orb first lit, before native microphone readiness, clipping the beginning of questions. Waiting for the native capture-ready event preserved both complete questions and returned 12 and 7. This corrects the test driver; it does not establish recognition accuracy across speakers or room conditions.

A separate Linux output check exposed sentence-tail truncation when HTML media was routed through `MediaElementAudioSourceNode`. PipeWire speaker-monitor capture contained only fragments of the three requested sentences. Keeping ordinary HTML media output on Linux preserved “Take a short walk. Stretch your shoulders. Drink a glass of water.” in an independent output recording, transcribed locally with the bundled Whisper model. The comparison used the same VM, voice and requested reply. Linux retains the smooth unmetered speaking animation; Mac and Windows retain their existing metered route. No microphone recording was used for that output-only comparison, and no additional cloud transcription was charged.

After the Linux playback change, a fresh microphone-ready two-turn run returned “Seven plus five is twelve” and “Ten minus three is seven”, then stopped on the spoken stop phrase without sending it to the model. A separate three-sentence reply completed and its quiet follow-up closed about 8.5 seconds after opening. The scoped synthetic chat contained no additional turn after that answer. All 14 production-WASM integration scenarios passed again, including in-flight start cancellation, mute, hide, Settings and playback/recording exclusion. WASM Clippy and formatting passed.

Full CI at `fe9f4c1` passed on all platforms, including Intel Mac and the Windows installer. That green result does not explain the earlier intermittent Intel decode deadline failure. The playback fix requires its own subsequent CI result. Detailed synthetic diagnostics remain local; credentials and incidental background transcripts are not published.

### CI coverage correction

At `b597f15`, the Intel Mac completed the main local validation suite, then failed the separate ignored helper lifecycle corpus with `Failure::Unavailable`. That extra CI command exercised a standalone executable which macOS neither packages nor selects: helper selection in the app is Windows-only, and the macOS resources contain the in-process model. CI now explicitly exercises the production Mac framing/corpus test in release mode instead. This covers all five positive/negative fixtures at both 16 kHz and 48 kHz with the unchanged three-second utterance deadline. Protocol/lifecycle unit tests still run, and Windows retains its guarded helper and native corpus checks. No production deadline, recognition guard or required platform job was relaxed.

The replacement command passed locally on Apple Silicon: ten expected matches/rejections, approximately 153–160 ms per decode. The separate earlier Intel in-process timing miss remains an observation; correcting this irrelevant helper check does not explain or claim to fix it. After a normal restart of the same Developer ID-signed Mac QA bundle, both saved voice keys were ready without another approval. The original QA profile was then restored and test apps stopped.

## Required before merge/release

- [ ] Real microphone → configured STT → Terra Medium → configured TTS exchange with two or more follow-ups, in click and wake modes.
- [ ] Verify the stop phrase by speaking; quoted/embedded phrases must not end an ordinary request.
- [ ] Hear the full streamed answer before the microphone reopens; verify no self-triggering in actual speakers/room conditions.
- [ ] Check no-speech timeout and speech beginning near the deadline without cutting off the request.
- [ ] Verify Escape, mute, hiding and opening Settings while listening and while an activation is in flight.
- [ ] Repeat hardware acceptance on Windows and Linux, including the chosen speech backends. Hosted CI establishes compilation/model/logic results, not device audio behaviour.

The existing 0.1.0 release and website claims are unchanged. Do not mark the roadmap item shipped until these voice checks pass.
