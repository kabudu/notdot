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

Completed on the rebuilt macOS QA app after local key approval, using known speech played through the speakers into the real microphone. This exercised wake detection → native streaming ElevenLabs transcription → Terra / Medium → speech playback → automatic confirmation capture → real memory persistence and retrieval. These were controlled synthetic utterances, not a human-speaker accuracy study.

| Check | Observed result |
| --- | --- |
| Automatic language, fruit sentence, negative reply | Correct fruit transcript; zero records before read-back and after rejection. |
| English language, same fruit sentence, negative reply | Correct fruit transcript; zero records before read-back and after rejection. |
| English peppermint-tea sentence, approval | First “yes, save that” attempt was rejected; the reason was not retained in diagnostics. A slower “yes, remember that” passed: zero records before approval, exactly one afterward. No permissive matching was added. |
| Later spoken recall | Retrieved the confirmed peppermint-tea note and answered correctly; no new memory question. One wake attempt missed; retry completed in 11.85 seconds from fixture playback start. |
| Bare “yes” | Discarded colour proposal; existing record count remained one. |
| Silence | Local capture ended, cancelled the proposal and resumed wake listening; no confirmation audio chunks sent; count remained one. |
| Escape during automatic confirmation capture | Scoped cancellation recorded; returned to wake listening; count remained one. |
| Settings memory review | Exactly the peppermint-tea note shown as voice confirmation / confirmed by you. |

Read-back turns took about 30–32 seconds including the model response, spoken verification question and spoken confirmation outcome. The extra check deliberately adds time when saving a durable fact. English and automatic detection both recognised the paired fruit sample correctly; this small comparison demonstrates no measured accuracy gain. Exact confirmation matching can reject legitimate approvals when recognition is imperfect, requiring a retry rather than risking an unintended save.

Ignored local evidence: `.local/stt-safety/{auto-reject,english-reject,english-accept,english-accept-slow,bare-yes-reject,silence-reject,escape-check}.json` and `.local/memory-e2e/stt-recall-retry.json`. No demo recording ran. The disposable profile is preserved at `.local/stt-safety/completed-test-profile`; the original QA profile was restored and reopened. Production file hashes match the pre-test snapshot. No application code changed after the passing CI/live-model checkpoint.

## References and limits

[ElevenLabs transcript API](https://elevenlabs.io/docs/api-reference/speech-to-text/convert) documents optional language hints, word metadata and decoding options; [realtime API](https://elevenlabs.io/docs/api-reference/speech-to-text/v-1-speech-to-text-realtime) documents streaming language selection. A language hint is not an accuracy/confidence guarantee. No word-error-rate improvement is claimed from this small controlled comparison. Read-back confirmation protects ledger storage from residual errors; it does not make ASR perfect or authenticate the speaker.
