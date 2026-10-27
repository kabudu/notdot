# Native voice acceptance — isolated QA only

Current native acceptance is blocked by the locked Mac and test-build keychain authorisation. Do not substitute the successful API/model/unit tests for this run.

## Prepared environment

- QA bundle: `target/debug/bundle/macos/notdot QA.app`.
- Bundle ID/profile: `com.kabudu.notdot.qa`; production remains `com.kabudu.notdot`.
- `scripts/prepare_qa.py` creates the profile once, copies only voice/model preferences, starts a new conversation, and leaves wake/startup/automatic updates/proactivity off. It refuses to overwrite existing QA data.
- Debug QA credentials permit existing-key reads/explicit authorisation, but reject saving or deleting OS credentials. QA startup changes and live update installation are also blocked. These restrictions prevent tests from changing shared OS state. No API key is copied to the QA settings file.
- Build: `cargo tauri build --debug --bundles app --config src-tauri/tauri.qa.conf.json`. This does not enable the debug text composer.

## Required native checks

1. Unlock the Mac, open the QA app and use Command/Control+comma to open Settings. Authorise the saved key locally if needed; never paste a key into a test report. Confirm the purple QA palette and isolated profile.
2. Check keyboard Tab/Shift+Tab order, focus rings, labelled dialogs/selectors/switches and VoiceOver names/status. With About open, Space must activate the focused control without starting the microphone. Check reduced-motion changes while running, narrow viewport/scrolling and multiple hue values.
3. Select/preview a voice, then record “What is seven plus five?” by orb click and separately by Enter/Space. Verify actual capture, transcription, correct spoken answer, and visibly distinct listening/thinking/speaking animation. Record wall-clock capture-stop-to-first-audio latency and listener confirmation. Inspect the QA `visual-status.json` only for mode/frame/RMS evidence; it is not proof that the speaker produced audible sound.
4. Cancel during capture, model waiting and playback. Confirm no missing-turnId error, late reply playback or stuck busy state. Test microphone denied/unplugged, network/provider errors and a deliberately unsupported saved model in the QA file, restoring supported settings afterward.
5. Temporarily name the QA companion Ava and enable “Hey Ava”. Confirm local activation, silence stop, no-speech discard, mute, pause during playback and recovery. Broader accents/names still require additional human samples.
6. Say “Remember that my synthetic test preference is teal notebooks.” Inspect, edit, confirm and forget in QA. Exercise Reactive/Suggestions/Proactive, permission, quiet hours and snooze with short synthetic reminders. Verify silence in Suggestions and permitted local speech in Proactive.
7. Reset **QA memory only**. Confirm restart, an empty QA ledger/reminder journal and a different thread ID while palette/voice/model/learning permission remain. Verify the old QA chat is not resumed. Never invoke reset on the user's production profile.
8. Actual OS sign-in startup, current Windows hardware acceptance (Windows App/RDP) and installed signed-update/restart acceptance remain separate gates. QA blocks live updater/OS-startup mutations; use dedicated local signed fixtures or a disposable OS account. No GitHub release or production installer publication is authorised by this checklist.

For each case retain: revision/build/profile, actual actions, expected/observed result, timing/failure count, and any screenshots/accessibility output. Keep secrets, real user recordings and real conversation content out of reports. Restore QA test settings afterward; leave production untouched.
