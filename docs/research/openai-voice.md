# OpenAI voice options

Checked on 4 November 2026. This is research, not an available notdot voice provider.

## What we have proved

**A silent WebRTC connection worked with the existing ChatGPT sign-in, without an API key.** The earlier failure applied to WebSocket transport; it did not rule out subscription-authenticated voice.

The bounded local proof used the installed Codex runtime, a fresh temporary read-only chat, Terra on Medium and disabled tools. It received both `thread/realtime/started` and an SDP answer, then established ICE and the data channel in about 2.7 seconds. Cleanup finished after 11.5 seconds. It used a silent synthetic track: no microphone, playback, user message or task execution. Sanitised evidence is kept locally at `.local/task-followups/webrtc-probe/evidence.json`.

This proves account-authenticated connectivity. It does **not** yet prove usable transcription, spoken replies, accuracy, latency, the amount charged to the account or a stable integration contract.

## Source and installed build

The installed ChatGPT application is version `26.1029.61003` (build `24614`). Its bundled runtime reports `codex-cli 0.166.0-alpha.6`. The voice manifest at `/Applications/ChatGPT.app/Contents/Resources/codex-cli/codex-resources/voice/manifest.json` declares commit `4f093f2748667f3b0e6a07e3d4055121cbd3ce64`, matching the official `rust-v0.166.0-alpha.6` tag. The bundled voice helper hash matched that manifest. This establishes declared source/version parity for that component, not a reproducible-build claim for the whole desktop application.

The pinned source explains the different results:

- [WebSocket alone requires an API key](https://github.com/openai/codex/blob/4f093f2748667f3b0e6a07e3d4055121cbd3ce64/codex-rs/core/src/realtime_conversation.rs#L1446-L1472). The earlier V2 and V3 WebSocket probes were acknowledged but failed before starting with “realtime conversation requires API key auth”.
- [WebRTC uses the normal authenticated client](https://github.com/openai/codex/blob/4f093f2748667f3b0e6a07e3d4055121cbd3ce64/codex-rs/core/src/client.rs#L810-L840). An [upstream test explicitly covers ChatGPT-authenticated V3 WebRTC](https://github.com/openai/codex/blob/4f093f2748667f3b0e6a07e3d4055121cbd3ce64/codex-rs/core/tests/suite/realtime_conversation.rs#L817-L965).
- [WebRTC accepts conversational sessions, not transcription-only sessions](https://github.com/openai/codex/blob/4f093f2748667f3b0e6a07e3d4055121cbd3ce64/codex-rs/core/src/realtime_conversation.rs#L1509-L1522). A working connection is therefore not automatically a separate STT/TTS service.

Static inspection also found the desktop shell's private `/wham/realtime/calls` route in its bundled JavaScript. We did not call that route, extract credentials or reuse private desktop tokens. The proof used the runtime's experimental `thread/realtime/start` interface instead.

## Subscription and support boundaries

OpenAI's [desktop pricing documentation](https://learn.chatgpt.com/docs/pricing#how-much-does-voice-cost) says desktop voice shares the existing Codex usage budget; backing tasks are billed separately within the plan's limits. It says desktop voice is not available through API-key authentication. The successful connection is consistent with that design, but we have not measured billing for a third-party client.

The [app-server interface](https://learn.chatgpt.com/docs/app-server) is intended for rich clients, but these realtime methods are experimental and absent from the ordinary generated schema. We have not found a documented, stable subscription-funded STT/TTS contract for third-party applications. Treat this as a promising prototype route, with version checks and a fallback, rather than a production promise.

## Public audio APIs

OpenAI also offers [streaming transcription](https://developers.openai.com/api/docs/guides/realtime-transcription) and [speech generation](https://developers.openai.com/api/docs/guides/text-to-speech) through separately billed API access. The transcription guide recommends `gpt-live-transcribe`; it emits partial and final transcripts but not word confidence scores. The speech guide now directs new work towards Realtime speech rather than the retiring speech models. API access should not be described as spending a ChatGPT subscription allowance.

## Permission boundary

V3 exposes incoming transcription events and an explicit `appendSpeech` operation, so separate recognition and fixed-text speech are plausible next experiments. However, `clientManagedHandoffs` is not an approval gate: the [runtime routes incoming delegation requests before forwarding their events](https://github.com/openai/codex/blob/4f093f2748667f3b0e6a07e3d4055121cbd3ce64/codex-rs/core/src/realtime_conversation.rs#L1814-L1823). That flag only changes how completed responses return to the voice session. A prompt cannot enforce notdot's permission checks. Production use needs an enforceable way to disable automatic delegation, or a maintained runtime change that provides it.

## Next proof

Test one synthetic utterance and one fixed spoken reply in an isolated, tool-free session. Verify transcript events, speech control, cancellation and whether the voice model can be prevented from starting its own backing tasks. Keep Terra on Medium as notdot's task model and retain notdot's own permission, speaker and settings checks. Do not enable this provider until those boundaries and real speech performance have been tested.
