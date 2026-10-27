# STT and voice-memory safety — 27 October 2026

Application checkpoint: `a0efdc1`. Terra / Medium is unchanged.

## Changes

- English is the default transcription-language hint; automatic detection remains available. Both realtime and batch ElevenLabs paths receive the setting. Batch requests retain verbatim speech, omit audio-event tags and specify temperature zero. No paid vocabulary/transcript-edit options or memory-derived recognition bias were added.
- New facts from the app's voice path stay in a bounded, in-memory proposal. No explicit or inferred voice fact enters the ledger before a scoped read-back is accepted with **yes, save that** or **yes, remember that**. Bare yes, negative/ambiguous responses, silence, cancellation, stale revisions and expiry cannot authorise a write.
- The prompt uses the selected voice service; its short answer capture starts automatically. New interactions supersede older proposals. Source evidence, policy and revision checks run before the question and before a confirmed write. Confirmed corrections retract older same-key facts and dependent summaries.
- Original speech transcripts still exist in external chat history, including rejected ones. Companion context marks them unverified and instructs personal-memory claims to rely on confirmed retrieved notes. Rejecting a ledger proposal is not chat-history erasure.

## Automated / live-model evidence

Full local CI passes for the checkpoint: native/core/provider lifecycle tests, default and debug WASM Clippy, gesture/visual/speech-queue tests, Python checks and formatting. Provider mocks verify language forwarding in the WebSocket query and batch multipart form, plus preservation of verbatim/no-audio-event options. Invalid language/query input is rejected.

Real-ledger native tests exercise no write before read-back, refusal before a question is offered, rejection of a misheard food/fruit proposal, positive single-use confirmation, confirmed correction, cancellation, expiry, memory-off and stale-worker invalidation. The authenticated native worker test also passes completed voice interaction → actual Terra analysis → pending proposal with an empty ledger → scoped confirmation → persisted source-linked fact.

The QA bundle builds and the staged secret scan passes. QA-only confirmation evidence records event/count fields without raw speech, proposed facts or keys.

## Native microphone acceptance

Pending local OS key approval for the rebuilt ad-hoc QA app. Planned checks: compare automatic/English recognition on known problem phrases; reject an incorrect read-back and verify zero records; approve a correct detail and verify later recall; reject bare yes; cancel/expire a confirmation; verify normal questions keep their streamed response and do not produce a memory prompt.

The original QA profile is preserved under ignored `.local/stt-safety/original-qa-profile`. The disposable profile has no user memories or conversation copied into it. QA Settings is open and wake listening is paused while the saved-key prompt waits. No demo recorder is running. Production data is untouched.

## References and limits

[ElevenLabs transcript API](https://elevenlabs.io/docs/api-reference/speech-to-text/convert) documents optional language hints, word metadata and decoding options; [realtime API](https://elevenlabs.io/docs/api-reference/speech-to-text/v-1-speech-to-text-realtime) documents streaming language selection. A language hint is not an accuracy/confidence guarantee. No measured word-error-rate improvement is claimed before the controlled microphone comparison. Read-back confirmation protects ledger storage from residual errors; it does not make ASR perfect or authenticate the speaker.
