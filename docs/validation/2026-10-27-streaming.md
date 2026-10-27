# Sentence speech implementation checkpoint

- Only turn-tagged `final_answer` items enter speech; commentary, reasoning and other turns are excluded.
- Cumulative text is capped at 64 KB. A cursor emits sentence chunks of about 600 bytes, with UTF-8-safe fallback for long unpunctuated text. Numbered-list labels and common abbreviations stay with their sentence.
- One worker plays clips in order and prefetches at most one next clip. Provider interfaces remain pluggable. This is sentence-level streaming, not byte-stream audio.
- Cancellation clears the local generation, cursor, pending identity and playback. Native request identities plus bounded, expiring tombstones handle cancel-before-dispatch; late cancellation cannot interrupt a newer request. Provider requests already dispatched may finish or incur charges, but late audio is discarded.
- Rewritten speech fails closed instead of repeating or silently changing already queued material. Stream and playback watchdogs bound stalled work.
- Per-playback gain-ramp listeners are owned and removed; a forgotten per-clip ended callback was removed.
- Production builds exclude the composer. The debug composer uses the same spoken pipeline for repeatable tests.

Validation: core event/cancellation tests, standalone queue tests, native/WASM Clippy and existing local CI pass at the preceding checkpoint. Native tests verified ordered three-sentence playback and active runtime interruption. The full local CI also passes with the wake-mode input guards and adaptive quiet-microphone gate. Replaying the private human recording reproduces the missed wake with the original fixed loudness gate; the updated detector accepts it at its original level and at 8×/32× gain. Existing unrelated-audio and name-without-Hey negative fixtures still pass. Final bundled live wake validation and the first-sentence pop remain open. See the full-voice report for actual observations and limitations.

Follow-up live wake check still failed. The rebuilt QA app showed “Allow saved key access”; native wake readiness requires `key_configured`, so local detection was paused before any microphone processing. The previous idle label incorrectly invited the user to say the phrase despite that gate. The orb status, tooltip and Hands-free message now identify missing key access, missing voice selection and actual detector state. Full local CI passes; OS key approval and live wake acceptance remain pending.
