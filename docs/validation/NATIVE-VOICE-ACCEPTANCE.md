# Native voice acceptance — isolated QA only

Real macOS microphone/reply tests have run. Full acceptance remains incomplete; see the dated full-voice and security reports. Do not substitute API/model/unit tests for the remaining native checks.

## Prepared environment

- QA bundle: `target/debug/bundle/macos/notdot QA.app`.
- Bundle ID/profile: `com.kabudu.notdot.qa`; production remains `com.kabudu.notdot`.
- `scripts/prepare_qa.py` creates the profile once, copies only voice/model preferences, starts a new conversation, and leaves wake/startup/automatic updates/proactivity off. It refuses to overwrite existing QA data.
- Debug QA credentials permit existing-key reads/explicit authorisation, but reject saving or deleting OS credentials. QA startup changes and live update installation are also blocked. These restrictions prevent tests from changing shared OS state. No API key is copied to the QA settings file.
- Build: `cargo tauri build --debug --bundles app --config src-tauri/tauri.qa.conf.json`. This does not enable the debug text composer.

For the synthetic provider API probe after authorisation, run the **same signed QA bundle executable**, not a separately built bare CLI binary: `"target/debug/bundle/macos/notdot QA.app/Contents/MacOS/notdot" --voice-smoke`. It reads that build’s QA settings, prints completed TTS/STT timings, and never requests an authentication prompt. This probe excludes model inference, microphone and speaker; it cannot replace the native steps below.

## Required native checks

1. Unlock the Mac, finish any active production turn and quit the production app to avoid two listeners. Open the QA app and use Command/Control+comma to open Settings. Authorise the saved key locally if needed; never paste a key into a test report. Confirm the purple QA palette and isolated profile.
2. Check keyboard Tab/Shift+Tab order, focus rings, labelled dialogs/selectors/switches and VoiceOver names/status.

With About open, Space must activate the focused control without starting the microphone. Check reduced-motion changes while running, narrow viewport/scrolling and multiple hue values.
3. Select/preview a voice, then say “What is seven plus five?” after one orb click and separately after Enter/Space. Pause to submit automatically; a second click can submit sooner. Verify actual capture, transcription, correct spoken answer, and visibly distinct listening/thinking/speaking animation. Record wall-clock capture-stop-to-first-audio latency and listener confirmation.

Inspect the QA `visual-status.json` only for mode/frame/RMS evidence; it is not proof that the speaker produced audible sound.
For wider speech coverage, use short requests that distinguish operators (plus, minus, times), digit sequences, “oat milk” versus “oatmeal,” an explicit correction and a half-second pause inside a sentence. Include a 20–25-second request, silence after activation and several consecutive turns. Check the actual transcript and meaning; an API request completing is not an accuracy pass. Memory approval/rejection scenarios belong only in the disposable QA profile. Generated fixtures complement these checks and do not replace the user's own voice.

4. Cancel during capture, model waiting and playback. Confirm no missing-turnId error, late reply playback or stuck busy state. Test lock-screen/occlusion behavior (including whether the microphone lease expires), microphone denied/unplugged, network/provider errors and a deliberately unsupported saved model in the QA file, restoring supported settings afterward.
5. Temporarily name the QA companion Ava and enable “Hey Ava”.

Confirm local activation, silence stop, no-speech discard, mute, pause during playback and recovery. Broader accents/names still require additional human samples.
6. Say “Remember that my synthetic test preference is teal notebooks.” Inspect, edit, confirm and forget in QA. Exercise Reactive/Suggestions/Proactive, permission, quiet hours and snooze with short synthetic reminders. Verify silence in Suggestions and permitted local speech in Proactive.
7. Reset **QA memory only**.

Confirm restart, an empty QA ledger/reminder journal and a different thread ID while palette/voice/model/learning permission remain. Verify the old QA chat is not resumed. Never invoke reset on the user's production profile.
8. Actual OS sign-in startup, current Windows hardware acceptance (Windows App/RDP) and installed signed-update/restart acceptance remain separate gates. QA blocks live updater/OS-startup mutations; use dedicated local signed fixtures or a disposable OS account.

No GitHub release or production installer publication is authorised by this checklist.

For each case retain: revision/build/profile, actual actions, expected/observed result, timing/failure count, and any screenshots/accessibility output. Keep secrets, real user recordings and real conversation content out of reports. Restore QA test settings afterward; leave production untouched.

Security scope: a wake phrase activates capture; it does not authenticate the speaker. Lock-screen/privacy behavior must be observed before release. The local memory ledger relies on application-directory permissions and OS storage protections; notdot does not claim to separately encrypt the ledger. External backups, cloud chat history and already-authorised provider requests are outside local reset/forget erasure. These boundaries must remain explicit in release claims.

## Native provider fixture helper

`cargo run -p notdot --example transcribe_fixtures -- SETTINGS_JSON SYNTHETIC_CORPUS_JSON`

This developer-only example reads the selected recogniser's existing local OS credential entry and streams up to 16 synthetic PCM16 WAV clips. The manifest is a JSON array of `path` and `expected` entries. It writes per-case transcripts and request outcomes to stdout, with a 45-second limit per request, and exits unsuccessfully if a provider request fails. It does not access the microphone, run the model, change conversations/memory, save keys or retry recognition. A completed request still needs its words checked against the expected phrase. Do not put real keys in arguments or reports. On macOS use the existing signed QA probe above when OS trust is required; do not copy a vault to another OS.

Cross-compiled native unit-test executables can set `NOTDOT_TEST_ROOT` to the target machine's staged `src-tauri` resource/fixture root. This override exists only in tests; packaged apps continue to use their bundled resources. Never enable the explicitly ignored credential/destruction tests against a real user profile.
