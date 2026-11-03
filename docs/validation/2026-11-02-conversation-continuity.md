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

## Required before merge/release

- [ ] Real microphone → configured STT → Terra Medium → configured TTS exchange with two or more follow-ups, in click and wake modes.
- [ ] Verify the stop phrase by speaking; quoted/embedded phrases must not end an ordinary request.
- [ ] Hear the full streamed answer before the microphone reopens; verify no self-triggering in actual speakers/room conditions.
- [ ] Check no-speech timeout and speech beginning near the deadline without cutting off the request.
- [ ] Verify Escape, mute, hiding and opening Settings while listening and while an activation is in flight.
- [ ] Repeat hardware acceptance on Windows and Linux, including the chosen speech backends. Hosted CI establishes compilation/model/logic results, not device audio behaviour.

The existing 0.1.0 release and website claims are unchanged. Do not mark the roadmap item shipped until these voice checks pass.
