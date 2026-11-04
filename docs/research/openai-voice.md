# OpenAI voice options

Checked against official OpenAI documentation on 4 November 2026. This is an integration assessment, not an announcement of a new notdot provider.

## Speech services exist

OpenAI provides [streaming transcription](https://developers.openai.com/api/docs/guides/realtime-transcription) and [speech generation](https://developers.openai.com/api/docs/guides/text-to-speech). They could fit notdot's separate speech-recognition and spoken-reply interfaces while Codex continues to handle tasks.

The transcription guide recommends `gpt-live-transcribe` for live audio. It streams partial transcripts and a final transcript for each committed turn, using client-side end-of-speech detection. It does not supply word confidence scores. Accuracy and latency would need the same microphone, noise and terminology tests as other providers.

The speech guide now marks `tts-1`, `tts-1-hd` and `gpt-4o-mini-tts` as deprecated, with shutdown on 6 January 2027. It directs new work to `gpt-realtime-2.1-mini` through the Realtime API. Do not start a new provider on the retiring `/v1/audio/speech` workflow.

## Subscription-funded desktop voice is a different product

OpenAI's [desktop pricing documentation](https://learn.chatgpt.com/docs/pricing#how-much-does-voice-cost) says Voice in Desktop uses the existing Codex usage budget, with voice and tasks sharing plan limits. It also says desktop voice is not available through an API key. The pricing documentation distinguishes API token pricing from subscription usage.

This is encouraging for a simpler experience, but it does not establish a supported third-party connection. The documented [Codex app-server interface](https://learn.chatgpt.com/docs/app-server) covers authentication, chats, approvals and agent events; the documentation reviewed does not describe a public subscription-funded transcription or speech bridge for notdot.

**Conclusion:** OpenAI can be a future voice provider using its documented audio APIs, but we cannot currently promise that notdot's speech will consume ChatGPT/Codex subscription credit. We have not demonstrated a supported subscription voice connection. Do not reuse private desktop endpoints or extract desktop credentials to make that claim.

## Installed runtime probe

The installed `codex-cli 0.166.0-alpha.6` can generate both ordinary and experimental app-server schemas. The ordinary request schema omits realtime voice methods; `--experimental` adds `thread/realtime/start`, `appendAudio`, `appendText`, `appendSpeech`, `stop` and `listVoices`.

A local capability query returned voice lists for V1, V2 and V3 without an API key. That is catalog availability, not working voice access. Two bounded silent session probes used a fresh ephemeral, read-only thread with tools disabled and the existing sign-in, first with V2/Marin and then V3/Cove. Both start requests were acknowledged, but neither emitted a started event. Both emitted `thread/realtime/error` with **“realtime conversation requires API key auth”**. Each session was stopped and its dedicated runtime terminated. No microphone recording, desktop credential extraction or task delegation was involved.

This establishes a concrete limitation of the tested runtime, not a claim that subscription-funded third-party voice will never be available. Do not confuse the JSON-RPC start acknowledgement or voice catalog with a successful audio connection. Recheck this boundary when OpenAI publishes a supported integration.

## Next step

Keep the OpenAI provider on the roadmap. Before implementation, recheck supported audio APIs and whether an official app-server voice integration has become available. Prefer that supported subscription route if offered; otherwise label API setup and billing clearly. A local speech backend is another way to remove a separate cloud voice account, with its own quality and hardware trade-offs.
