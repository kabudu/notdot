# Android wake validation

The Android native recogniser is an experiment, not an accepted replacement for the current wake detector. These results were obtained on a physical Motorola phone running Android 14. No paid speech service or API key was used.

## What works

- The app can send captured PCM16 audio to Android's explicitly on-device recogniser. It never creates the generic recogniser, downloads a language model or switches to cloud recognition.
- The adapter handles cancellation, stale callbacks, duplicate closing results, platform failures and bounded startup waits. Thirteen JVM tests pass, including a regression test that fails when the old re-entrant cancellation bug is restored.
- Short supplied-audio recognition tests return results while the phone is locked and its screen is off.
- An empty language-support response no longer incorrectly prevents startup. On the tested phone, English (UK) returns results; English (US) reports language unavailable.

## Why it is not enabled by default

| Check | Observed result |
| --- | --- |
| Three synthetic voices saying “Hey Ava” | All three top results match |
| Nine near-name negatives across those voices: Ada, Eva and Nova | One top result falsely says Ava |
| Three existing public wake recordings | The target appears among alternatives, but the top results do not match |
| Accepting any alternative instead | Adds a false activation on Eva; rejected |
| No vocabulary hints, target hints, or balanced near-name hints | Same result on the compared clips |
| Recognition confidence | All returned scores are zero; they cannot distinguish a correct wake from the false activation |
| Exact adapter on the original public clip | One segment, four partial results, zero wakes, no dropped bytes or recogniser error |
| Supplied-audio recognition after a minute of silence with the phone locked | No result within the test deadline; continuous microphone-service operation remains unverified |

The public clip's top spelling is “Hey Aver”. Looser spelling or alternative matching would improve recall but does not resolve the observed Ada false activation. The shipped matching rule has not been relaxed to manufacture a pass.

The long-idle test uses paced supplied audio, not an acoustic recording. Screen-off scheduling can delay that feeder. Its timeout is not proof that an already-running microphone foreground service fails, and short successful results are not proof of overnight reliability. Battery use, Doze, real acoustic latency and sustained false-wake rates remain acceptance gates.

## Reproduce

Build the experimental app without the Whisper backup:

```sh
NOTDOT_WHISPER=0 mobile/android/scripts/build-core.sh release
cd mobile/android
./gradlew -PnotdotNativeWake=true :app:assembleDebug :app:assembleDebugAndroidTest
./gradlew -PnotdotNativeWake=true :app:testDebugUnitTest --tests '*OnDeviceWakeRecognizerTest*'
```

Install both generated APKs on an authorised Android 13 or newer device with microphone permission. Run `PlatformWakeProbeTest` with Android's instrumentation runner. `adapterRecognisesPacedFixture` exercises the actual adapter and asserts a wake; it currently fails on the tested phone. The SDK-level diagnostic separately reports whether the top result or another alternative matches. That diagnostic is not an end-to-end wake acceptance pass.

The `fixture`, `gain`, `biased` and `idleSeconds` instrumentation arguments allow controlled comparisons. Additional synthetic near-name fixtures used in the measurements are generated locally, not bundled with the app or committed. No personal recordings, transcripts, device identifiers or connection details are retained in this document.

The existing detector remains the default. Set `-PnotdotNativeWake=true` only for this experiment; no fallback decoder runs in that path. Mobile support remains a draft and these results do not change desktop wake behaviour.
