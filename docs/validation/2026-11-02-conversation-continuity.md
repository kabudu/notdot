# Conversation continuity: implementation and acceptance

Status: feature branch `codex/conversation-continuity`; not released. Real voice testing is deferred at the owner’s request until the next voice session. No microphone, speaker, saved credentials or live speech/model services were used for the UI integration checks below.

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
- Browser integration harness: actual built production WASM interface with explicitly synthetic Tauri IPC, capture, STT, model and audio adapters. It covers click- and wake-initiated exchanges, repeated turns, stop routing, no reopening after stop/timeout, Escape, mute, hiding and settings, in-flight activation cancellation, settings-control persistence, and capture exclusion during playback and inter-sentence gaps. All 13 browser scenarios passed. This is integration simulation, not real microphone-to-model-to-speaker evidence.

Reproduce the silent browser checks with:

```sh
trunk build --dist .local/continuity-ui
python3.12 scripts/prepare_conversation_harness.py
python3.12 -m http.server 8095 --bind 127.0.0.1 --directory .local/continuity-ui
```

Open `http://localhost:8095/` and choose **Run silent integration checks**. The harness installs virtual audio adapters before loading WASM and rejects browser microphone requests. It uses only staged test phrases, no keys and no external service requests. Rebuild before preparing it again. Test adapters are not included in the app bundle.

## Required before merge/release

- [ ] Real microphone → configured STT → Terra Medium → configured TTS exchange with two or more follow-ups, in click and wake modes.
- [ ] Verify the stop phrase by speaking; quoted/embedded phrases must not end an ordinary request.
- [ ] Hear the full streamed answer before the microphone reopens; verify no self-triggering in actual speakers/room conditions.
- [ ] Check no-speech timeout and speech beginning near the deadline without cutting off the request.
- [ ] Verify Escape, mute, hiding and opening Settings while listening and while an activation is in flight.
- [ ] Repeat hardware acceptance on Windows and Linux, including the chosen speech backends. Hosted CI establishes compilation/model/logic results, not device audio behaviour.

The existing 0.1.0 release and website claims are unchanged. Do not mark the roadmap item shipped until these voice checks pass.
