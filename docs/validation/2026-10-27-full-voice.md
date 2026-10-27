# Full voice test — 27 October 2026

QA only, macOS, Terra/Medium, memory and wake listening off. No promotional recorder ran. Initial build: ae67e41.

## Initial human microphone turn

The owner spoke and confirmed hearing the reply. The saved runtime input was only “Five”; the answer was “Five times seven is 35.” This did not pass transcription accuracy: the owner intended addition. The request audio is not retained by notdot, so the exact cause of the lost words cannot be recovered from this run. The old conversation also contained multiplication context. New companion instructions ask for clarification when a short utterance is incomplete instead of guessing a missing arithmetic operation.

Renderer telemetry recorded 5.248 seconds in listening, then 14.206 seconds from the first post-capture thinking frame to playback, and 2.540 seconds of playback. That is stop-to-playback-state latency, not measured first audible phoneme latency. It includes transcription, runtime work, polling, synthesis and playback startup. It is one warm QA sample, not a latency benchmark. The owner confirmed actual speaker output.

A 0.367-second idle gap appeared between transcription and model waiting. The renderer now treats an armed reply as thinking even when its latest runtime snapshot still says idle. Source inspection also found that a late successful submission could arm playback after Escape; the generation is now checked again after that await.

## UX changes prompted by the run

- One click begins capture; 1.4 seconds of silence after detected speech submits it automatically. A second click remains an early-submit shortcut, not a requirement.
- Silence/click noise without sufficient speech is discarded after eight seconds. Capture remains bounded to sixty seconds. Missing metering fails rather than uploading an unmonitored recording.
- Listening now uses one smooth closed shell and speaking uses two; the remaining radial microphone spokes were removed. Both use the smoothed audio envelope.
- Keyboard help and the user/developer guides describe one-click use.

The endpoint detector uses local audio level and duration, not semantic understanding or guaranteed speech/noise classification. Very quiet speech or continuous background noise still needs broader real-environment testing. Full retest evidence follows after local OS approval of the rebuilt QA app.

## Retest checkpoint

On the one-click QA build from 27b3c89, capture cancellation passed: a click reached Listening and Escape returned to Ready without a submitted turn or late playback. The saved key was authorised locally. A new one-click human request was requested but no submitted turn was observed in the bounded three-minute watch; transcription accuracy, automatic submission and model-wait cancellation remain pending live input. No success is inferred from the timeout.

The final security bundle from a8bc6c7 builds successfully. Its additional local-tool restrictions were verified separately in the real-runtime canary and denied-MCP-write probes. The currently running QA process remains the one-click build for the requested spoken retest; restart into the final bundle after that retest. Production data/settings were untouched.

## Successful one-click microphone turn

The owner confirmed automatic submission after a single click and the correct spoken reply. The stored transcript was “Play five plus seven and answer shortly.” and the answer was “12.” The wording was imperfect, but the addition request was preserved.

Telemetry recorded listening → thinking → speaking → idle, without the earlier intermediate idle flash. Listening lasted 5.310 seconds, including the end-of-speech pause; stop-to-playback-state latency was **4.332 seconds**, followed by 1.175 seconds of playback. Audio was metered and the owner confirmed actual speaker output. This is one successful warm sample and is not a performance improvement benchmark against the earlier, incorrectly transcribed request.

Microphone capture cancellation has passed; model-wait cancellation is being tested separately. No promotional recorder ran.
