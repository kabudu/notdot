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

The regression reproduces the discarded-error behaviour with the old policy and passes with the correction. Local CI passes 110 Rust tests, 10 Python checks, native/production/debug WASM Clippy and formatting. A native Windows test executable and model/fixture bundle are staged, but execution and installation of the corrected production app are still pending desktop-control access. The corrected production executable and unsigned NSIS installer have also built successfully; the staged executable SHA-256 is `46039fd2ef6af431a244221d22d2066717e53609008ff7b44c75db5c96587292`. They have not yet been installed in Windows. These passes are not a successful acoustic rerun of the failure above.

## Remaining checks

Repeat the failed acoustic request with the corrected diagnostic, then cover repeated turns, silence, pauses, wake activation, microphone changes and the user's natural speech. The user is unavailable for a live voice session at this point. Desktop tests were interrupted by a Mac lock and then loss of the desktop-control integration after the app refreshed its tools. Neither condition is a product test failure or a reason to mark unperformed checks as passed. No demo is being recorded.
