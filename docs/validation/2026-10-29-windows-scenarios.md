# Windows speech scenarios — 29 October 2026

Testing is in progress. These results extend the [earlier native Windows checks](2026-10-29-unattended.md), without establishing full voice or release acceptance.

## Boundary checks

The installed normal Windows app was the production executable with SHA-256 `fcf173ba0eb0dee9c571f9b134e96a3d7ca0729539d8cc70ef19954bb1a18531`, saved Terra Medium, Deepgram recognition and ElevenLabs speech. A generated Daniel voice asked “What is fourteen minus six? Please answer briefly” through the Mac speaker and forwarded Windows microphone. The app entered processing but displayed a generic Deepgram recognition failure. No subtraction turn reached the model; `thread/read` still showed seven existing turns. This is a failed acoustic test, not evidence of an invalid key. The exact underlying cause remains under investigation.

The latest previously cancelled compiler request was independently reported as `interrupted` by the runtime, supplementing the earlier timed playback-state check.

## Recognition-only comparison on Windows

A separate developer example used the same Windows user's saved Deepgram credential, without printing/exporting it, and streamed nine synthetic WAV fixtures directly to Flux. It neither opened the microphone nor called the model, spoke a reply or changed memory. The actual app was closed during this comparison.

All nine provider requests completed, with one final segment and partial events for each. Eight preserved the intended words (ignoring case and punctuation). One changed “save” to “say.” This does not establish general accent/noise accuracy.

| Scenario | Observed result |
| --- | --- |
| Five plus seven | Correct |
| Five times seven | Correct |
| Yes, save that | Correct |
| No, do not save that | “No. Do not say that.” — lexical error; not an affirmative approval |
| Not Tuesday, Thursday at half past three | Correct |
| One five nine, not one nine five | Correct |
| Oat milk, not whole milk | Correct |
| Addition with mild rumble | Correct |
| Digit sequence with heavy rumble | Correct |

Elapsed request times were 3.842–6.104 seconds, including paced upload of the entire fixture, endpoint silence and connection setup. These are not response latency or a performance comparison. Each request had a single recognition pass; no premium hints, batch retries or provider switching were used.

The helper executable SHA-256 was `8766adf879c4266ae3212d2b8dd5d2f46a56eee156abebdc6c1718ac8fa3943f`. Private synthetic clips and generated binaries remain outside version control.

## Error handling correction

Inspection found that the streaming manager discarded non-authentication/non-quota failures into the batch-fallback path. Deepgram has no batch path, so the UI lost the real reason and suggested checking the saved key. The manager now preserves streaming-only failures; authentication, permission, quota, rate-limit and invalid-input failures cannot trigger batch retries. Diagnostics distinguish an actual retry from a failed request. The caller also no longer suggests an invalid key merely because a streaming session was absent.

The regression reproduces the discarded-error behaviour with the old policy and passes with the correction. Local CI passes 110 Rust tests, 10 Python checks, native/production/debug WASM Clippy and formatting. The native Windows app test executable has since run, with the failures recorded below; installation of the corrected production app was still pending at that stage. The corrected production executable and unsigned NSIS installer have also built successfully; the staged executable SHA-256 is `46039fd2ef6af431a244221d22d2066717e53609008ff7b44c75db5c96587292`. Installation was subsequently verified below. These passes are not a successful acoustic rerun of the failure above.

## Remaining checks

Desktop control returned after restarting Codex. The native Windows app regression executable then ran with `NOTDOT_TEST_ROOT` pointing to the extracted synthetic fixtures: **41 passed, 2 failed, 6 ignored** (49 total; 66.06 seconds). This is the native-app test binary, not the entire workspace suite. The streaming error-policy regression passed. The failures were `bundled_phrases_reject_name_only_unrelated_audio_and_cancelled_work` and `wake_after_extended_idle_and_rejected_candidates`. The first returned a Whisper encoder error (`-6`); the second missed `hey-ava.wav` after two seconds of idle. A focused rerun reproduced the encoder error (13.62 seconds including model loading). The encoder failure was not yet explained at that point; the subsequent diagnosis below identifies the deadline. Windows wake acceptance remains open. Logs remain in the Windows user's temporary directory as `notdot-native-tests.log` and `notdot-wake-failure.log`.

### Subsequent diagnosis

The corrected Windows production executable was installed, its SHA-256 verified as `46039fd2ef6af431a244221d22d2066717e53609008ff7b44c75db5c96587292`, and its normal startup observed after Avast completed its scan. Repeating the generated subtraction phrase through the speaker and forwarded microphone still failed, but the UI now preserved the actual reason: **“Deepgram final transcript timed out.”** This is not a successful voice turn and does not establish why the provider returned no final transcript.

A bounded, recognition-only Windows diagnostic confirmed that the wake encoder error was caused by the existing three-second deadline. With a fifteen-second diagnostic budget, the same phrase was recognised successfully in 11.422 seconds after loading the model. A smaller audio context recognised this one phrase in 2.117 seconds, but subsequent Mac corpus checks rejected that optimisation: contexts of 384, 512, 640, 768, 896 and 1024 all missed the US-accent fixture (four of five correct), while the full context preserved all five expected decisions. The full production context and three-second budget are retained. CPU acceleration is disabled in this cross-built Windows recognizer; its contribution still needs a controlled comparison. No hardware requirement has been changed.

The recognizer now distinguishes its own timeout from a generic Whisper error, with a zero-budget regression that fails under the old error policy and passes under the correction. Local CI passes 110 Rust tests, 10 Python checks, formatting and native/production/debug WASM Clippy. A developer-only ignored diagnostic compares full-context attention and thread settings; its output must be checked for correct decisions, not treated as acceptance merely because the diagnostic finishes. Windows wake reliability and the acoustic timeout remain unresolved.

Repeat the failed acoustic request with the corrected diagnostic, then cover repeated turns, silence, pauses, wake activation, microphone changes and the user's natural speech. The user is unavailable for a live voice session at this point. Desktop tests were interrupted by a Mac lock and then loss of the desktop-control integration after the app refreshed its tools. Neither condition is a product test failure or a reason to mark unperformed checks as passed. No demo is being recorded.

## Native wake regression acceptance — 30 October

The self-contained `dede90f` candidate was copied to the laptop's local application-data directory. Its generic parent passed all 18 CPU/OS capability checks before testing the separately guarded accelerated helper. The helper executable SHA-256 is `63d8f978316bcc11c5341b6b08e50ae0448f7cfb85863cc75b9d33c1e859d402`; the app executable is `7940e343f7de2e51df912f2ad24a0c7ff52de48558c386b7d411e1a802200756`.

Native execution on the laptop passed:

- Three helper lifecycle tests, including cancellation, timeout and confirmed cleanup before installation: 18.28 seconds.
- The separately enabled five-fixture model comparison through one persistent helper: 8.04 seconds.
- The full app test executable, run serially: **50 passed, zero failed, seven explicitly ignored**, 69.75 seconds. The two previously failing wake tests now pass.

These are real Windows model/lifecycle tests using synthetic fixtures, without microphone or provider calls. They close the earlier offline wake failures; live microphone, wake activation and installed-update acceptance remain separate checks. The console pipeline was required to capture the GUI-subsystem test executable's output; the initial direct invocation produced no usable result evidence.

## Forwarded microphone investigation — 30 October

A fresh acoustic subtraction attempt still failed on `9cb7609`: Deepgram reported no speech updates before finalisation timed out. A metadata-only microphone diagnostic reproduced roughly 46–48% audio coverage both in quiet and during the generated phrase. Explicitly foregrounding Windows App did not correct it. The channels were identical, so stereo cancellation was not the cause.

A direct WASAPI comparison then measured actual device-position gaps, without reading or saving waveform payloads. With the default 970-frame ring, it delivered 2,559 ms of audio in 5,021 ms and reported 105,068 missing frames and 213 discontinuities. With a 100 ms/4,410-frame ring, it delivered 4,960 ms in 5,022 ms, with no missing frames or discontinuities. The endpoint remained 44.1 kHz stereo float throughout. This establishes lost packets in the small-buffer capture path, rather than proving a sample-rate mismatch.

The same comparison through CPAL reproduced the result: the default delivered 2,375 ms in 5,100 ms with 215 underrun/overrun notifications; a 4,410-frame ring delivered 4,920 ms in 5,074 ms with zero errors. Unsupported 22.05 kHz/16 kHz mono formats were skipped rather than forced. The app now requests this 100 ms ring on Windows while preserving the device's default sample format, rate, channels and callback period. A rejected buffer configuration gets exactly one attempt with the original configuration; permission, device and other failures are not retried. Mac/Linux retain their original configuration.

Three focused builder-boundary tests and Clippy pass. Independent source review found no actionable issue in the buffer ownership, fallback or allocation bounds. The updated Windows executables have built; their native regression and full acoustic acceptance remain pending at this checkpoint. This fix does not add a transcription request, change the model, or loosen speech thresholds.


## Updated Windows voice acceptance — 30 October

The `66587d1` production executable was verified on the laptop with SHA-256 `629e36dca9a192d0c6234cd518d648f1ed51caee9ebc956c68c772ee87a28b16`. Its native app suite passed **53 tests, zero failures, seven ignored**, in 70.32 seconds.

Generated speech travelled through the Mac speaker, forwarded Windows microphone, Deepgram, Terra Medium and ElevenLabs playback. The subtraction fixture reached thinking and spoken playback; its exact transcript/final text was not independently read in this pass. The read-only oracle verified these subsequent public results:

- “Please calculate five plus seven.” → “Twelve.”
- “What is five times seven?” → “Thirty-five.”

The oracle confirmed the actual model and medium effort. A mistimed operator fixture supplied only “seven.” and was excluded from successful recognition results.

Capture cancellation and no-speech activation added no model turn. During the compiler-response test, Escape returned thinking to idle in **225 ms**; the persisted turn was interrupted, with no public final or subsequent speaking transition. **Playback cancellation is not yet verified.** The interrupted-turn oracle can mistake Codex's injected abort context for spoken input; that field was excluded from the cancellation evidence.

Reduced motion remained enabled. Current checks used generated fixtures and playback/state metadata; earlier human RDP voice passes do not establish fresh human hearing or full-motion animation acceptance. Live wake, device/startup and installed-update gates remain separate.

## Normal Windows candidate — 30 October, `3ca1b45`

The locally copied normal app and helper hashes matched the candidate manifest. Native tests passed **55/55** (seven ignored) in 82.78 seconds; helper lifecycle tests passed **3/3** in 1.59 seconds, and the persistent five-fixture helper corpus passed in 13.75 seconds. The initial test setup used the old helper inside the fixture directory and correctly failed the accelerated-backend assertion; replacing that fixture helper with the matching candidate resolved the setup failure. Avast scanned the new binaries and reported them safe; no security settings were changed.

A generated spoken subtraction request traversed the forwarded microphone, Deepgram, Terra Medium and ElevenLabs playback. The public answer was “Fourteen minus six is eight.” The transcript retained the intended numbers and operator, but misheard the opening “What is” as “Forty is”; this is a pipeline pass, not perfect transcription. Recording ended 421 ms after the fixture, and speaking began 4.793 seconds after it. Two earlier mistimed fixture attempts produced no model turn and were excluded.

The compiler request was transcribed correctly and produced a ten-sentence public answer. During active playback, the observed audio meter was nonzero (RMS 0.068); focusing notdot and pressing Escape returned it to idle within that keyboard call, with no subsequent speaking transition in the later check. This verifies playback cancellation through state/output metadata, without fresh human hearing confirmation. Reduced motion remained enabled. Optional live wake was not repeated. Test environment variables were removed before the normal app launched; app settings were unchanged.

These checks cover the `3ca1b45` build. Later settings-copy changes require fresh assets in the final package and were not visually tested here.

## Hands-free repeat and final UI package — 30 October

Root completed two consecutive acoustic hands-free turns on the `3ca1b45` backend with the companion name `ava` in lowercase. “Please calculate five plus seven.” produced “Twelve.”; “What is five times seven?” produced “Thirty-five.” Both transcripts were exact, both public turns completed on Terra Medium, and playback output was nonzero. Activation followed the end of the wake phrase by **631 ms** and **731 ms**; speaking followed the end of each request by **1.833 seconds** and **1.201 seconds**. These are two generated speaker-to-forwarded-microphone fixtures, not a broad accuracy benchmark or fresh human hearing confirmation. The original name `notdot` and wake-off preference were restored and reopened for verification.

The final copy-only `cd01d8b` package was then copied to a new local folder and launched normally. The previous app quit through its tray menu; no notdot process remained before launch, and exactly one afterward. The running path was `C:\Users\Kamba\AppData\Local\notdot-candidate-cd01d8b\notdot.exe`. The laptop app SHA-256 matched `56aedb23ac80b0272957f0f831531cb07f5cfb69c2b6a66be8890e0921c9006c`; the helper matched `b0b6dfc3b4b9b0c4489e4da595652822bfedb60e528499776900e6e72828543a`. All 322 packaged notice files were present.

The scoped final UI check verified the updated saved-key placeholder, “Allow saved Deepgram key access”, “Deepgram is ready.” and plain secure-store guidance. The earlier developer validation message was absent. Settings closed without saving or changing keys, name, model or wake preferences; no test environment variables remained. No audio or provider request was used for this copy-only check. The native and voice evidence above belongs to the unchanged `3ca1b45` backend; it was not unnecessarily repeated for settings wording alone.

## Window restoration — `40ce478`

The final window-only fix was copied to `C:\Users\Kamba\AppData\Local\notdot-candidate-40ce478\notdot.exe`. Its laptop SHA-256 matched `44f7e4d09e4458945db820f7708ed2211ad506b8d5c1e1bb0b13c968d58839c3`; the unchanged helper matched `b0b6dfc3b4b9b0c4489e4da595652822bfedb60e528499776900e6e72828543a`. The preceding app quit normally through its tray; no notdot process remained before launch.

With Settings focused, clicking the Windows taskbar button minimized the window. Tray → Settings restored the visible settings panel. Minimizing again and launching the same executable restored the existing panel; the process check showed exactly one notdot instance (PID 28352) at the new path. Settings then closed without saving changes. No audio, key or preference changes were used for this scoped check. About and the optional startup-registration cycle were not repeated; OS sign-in acceptance remains separate. The `3ca1b45` voice/native results above remain the backend evidence and were not repeated for this window-only change.

## Startup registration — `40ce478`

The original “Start when I sign in” preference was off, and querying only the `notdot` value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` confirmed it absent. Enabling the switch and saving created a `REG_SZ` value pointing to `C:\Users\Kamba\AppData\Local\notdot-candidate-40ce478\notdot.exe --startup`. Reopening Settings confirmed the enabled preference persisted. Disabling it and saving removed that exact value; a fresh Settings view confirmed the original off/manual preference persisted. Settings then closed. No other preferences, credentials or security settings changed. This verifies registration and removal, without claiming launch after an actual Windows sign-in. No audio or provider request was used.
