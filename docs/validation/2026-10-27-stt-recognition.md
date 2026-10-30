# Speech recognition follow-up — 27 October 2026

## Scope and result

Tested 28 distinct synthetic clips through the real ElevenLabs APIs: 12 clean utterances with Daniel/Samantha voices at 150–230 words/minute, eight with added low-frequency rumble, four with heavier rumble, and four mixed with a quieter competing speaker. Cases include addition/multiplication, preferences, corrections, negation, numbers, project names, fast speech and short approvals. Both realtime and full-clip decoding were evaluated. These are controlled synthetic fixtures, not a human/accent/noise benchmark or a calibrated word-error-rate study.

The earlier rejected approval was reproduced in clean audio: both decoders returned “yes, say that” for Daniel's short “yes, save that”; “yes, remember that” was recognised correctly. The spoken memory question now asks for the latter. Strict matching still refuses “say that”, bare yes and negative/ambiguous answers. No positive-match relaxation was introduced.

Rumble conditioning improved the signal but not the words in this set. The provider's background-speech filter also did not correct the remaining competing-speaker error (“oat milk” became “oatmeal”). Both experiments were removed. Full-clip decoding correctly returned “oat milk” in those trials, but adds a request, usage and measured 0.4–0.9 second delay. The user rejected that latency and the premium vocabulary-hint option. Neither option is enabled, and no overall ElevenLabs recognition gain is claimed.

## Wake reproduction and acoustic acceptance

A reproducible wake phrase with a roughly 250 ms internal pause failed with the previous 200 ms fallback segmentation. With 350 ms, it passed (1.67 seconds in the local streaming fixture test). A regression test prevents splitting a short internal pause into an incomplete greeting. The fast keyword detector and exact configured-name matching are unchanged. This reproduces one missed-wake mechanism, not proof that every intermittent miss had that cause.

The real microphone path also accepted that paused phrase, transcribed “Please calculate five plus seven”, kept Terra/Medium and spoke “Five plus seven is twelve”. Capture began at 1.94 seconds and the app returned to listening at 11.81 seconds from wake-fixture playback start. Actual streaming mode and partial events were recorded. These acoustic checks ran before the separate-recognizer wiring, during the later-discarded background-filter experiment; the final wiring still needs the next native acceptance run on both backends. Two normal-rate acoustic memory approvals with the clearer phrase saved the intended tea/fruit details only after read-back. The original missing activation was intermittent; these bounded passes are not a universal detection guarantee.

Local asynchronous wake tests passed for the paused phrase and the previously captured acoustic fixture. “Hey avocado”, “Hey avalanche” and a greeting without the name did not activate it. The offline fixture helper now drains the asynchronous fallback worker with a bounded wait instead of racing its result.

## Alternative backend prepared; live gates deferred

Deepgram Flux is opt-in under Settings → Voice → Speech recognition; the default remains the existing voice service. A separate Deepgram key field writes only through the native credential boundary. QA uses a separate recognition-key service, while production ElevenLabs credentials remain protected. No key was supplied or read for Deepgram during this session.

The adapter uses one English Flux stream, sensitive-header authentication, bounded mono PCM16 upload, committed EndOfTurn text only, explicit native endpointing and cancellation. Native endpointing is retained initially; the provider's automatic/eager turn detection is not used or claimed as a speed improvement. Native finalization uses ForceEndTurn only after request endpointing; Flux skips the ElevenLabs path's arbitrary 20-second segmentation because that can finalize an incompletely decoded word. Provider silence timeout is 60 seconds, matching the maximum native request window. Failed streams do not send audio to another provider or use a second decoding pass. Unsupported microphone rates fail explicitly. Speech playback remains on the selected voice service.

Mock WebSocket tests pass for authentication location, binary chunk bounds, explicit finalization and rejection of interim rewrites. Settings migration/English validation, credential-service separation, native/WASM compilation, Clippy, full local tests and static accessibility checks pass. This does not establish actual Deepgram protocol/account compatibility or recognition quality.

The user asked to stop voice testing until tomorrow. Remaining gates: enter the key locally; visually verify native key setup; run the same corpus on the real Deepgram API; compare recognition and finalization latency; then verify microphone → Terra/Medium → ElevenLabs reply, negatives, cancellation, silence, long speech and memory confirmation. Do not claim improved accuracy, provider-driven endpointing speed, or completed Deepgram E2E before those checks.

## Evidence and cleanup

Ignored evidence is in `.local/stt-accuracy`: baseline/hard/after/conversation-before/filtered JSONL results, wake-before/after/final logs and bundle/CI logs. Acoustic confirmation results are `.local/stt-safety/accuracy-approval.json` and `final-approval.json`. The selected-recognizer debug CLI accepts a bounded synthetic corpus manifest and outputs transcripts only for that explicit diagnostic; it never uses chat history or memory. No private recordings, keys or local result files were added to Git.

The original QA profile was restored; the disposable speech-test profile is preserved under `.local/stt-accuracy/completed-test-profile`. Production hashes match the pre-test snapshot. QA is closed, no demo recorder is running, and no further sound or microphone tests ran after the user's stop request. Website cleanup was separately committed as `8d33a56`, checked structurally and visually at desktop/mobile sizes, and kept local at port 8094.

## References

- [ElevenLabs realtime options](https://elevenlabs.io/docs/api-reference/speech-to-text/v-1-speech-to-text-realtime)
- [ElevenLabs full-clip transcription](https://elevenlabs.io/docs/api-reference/speech-to-text/convert)
- [Deepgram Flux connection and format requirements](https://developers.deepgram.com/docs/flux/quickstart)
- [Deepgram external finalization semantics](https://developers.deepgram.com/docs/flux/force-end-turn)
