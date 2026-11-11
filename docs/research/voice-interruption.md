## Current candidate: unprefixed speech interruption

The v0.8.0 candidate now uses Silero speech detection on the webview's echo-cancelled audio, not wake-word detection. It retains a half-second pre-roll, pauses on speech onset and keeps the same microphone stream until 600 ms of silence (maximum 12 seconds). The selected streaming recognition backend receives the completed bounded utterance in a single session; this path currently starts recognition after endpointing, rather than uploading ongoing overlap audio. Only the original reply owner is admitted, and cancellation revokes admission. No raw audio is stored on disk. Two lifecycle regressions, native onset/buffer/owner tests and simulated production-WASM interruption checks cover software behaviour. They do not establish real-person overlapping speech reliability. External-person overlap and Windows/Linux webview capability checks remain pending.

The findings below describe the earlier wake-only investigation and its limitations, not the current trigger design.

# Voice interruption: design note and findings

Status: not shipped in 0.8.0. This note records what was tried and what a supported version needs.

## Goal and scope

The roadmap goal is to stop a spoken reply by speaking, without notdot reacting to its own speaker output. Two scopes were considered:

- **Unprefixed interruption.** Say "Stop" or "Actually, ..." while notdot speaks. This needs reliable separation of a person's voice from notdot's own playback, and is the long-term goal.
- **Wake-phrase-first interruption.** Say the wake phrase while notdot speaks, then "stop", a correction or a new request. This keeps the local wake detector as the only listener until the phrase is heard, so no audio leaves the computer before then.

## What the current capture path can do

Wake listening uses native capture (cpal) with an on-device keyword spotter and a fallback decoder. Neither path cancels acoustic echo, and voice activity detection is not echo cancellation.

Synthetic measurements used the licensed Piper fixtures, mixed digitally. The mix has no room acoustics, output latency or loudspeaker distortion, and no real person was recorded. Diagnostics: `cargo test -p notdot --release --bin notdot -- wake::tests::interruption --include-ignored --nocapture`.

| Case | Result |
| --- | --- |
| Reply audio alone, echo 1% to 100% of full scale | No activation |
| The name alone over reply audio | No activation |
| A reply sentence that is itself the wake phrase, 1% to 100% | Activates at every level |
| Wake phrase (35%) overlapping reply audio at 1% or 2% | Detected |
| Wake phrase (35%) overlapping reply audio at 5% to 100% | Missed by both detectors |
| Wake phrase (80%) overlapping reply audio at 20% | Missed by both detectors |
| Wake phrase in a pause between reply sentences | Detected |
| Wake phrase directly after reply speech, no overlap | Detected by the fallback decoder only |

The keyword spotter produced no candidate under overlap, so this is not a threshold or decoder-window effect. Relaxing thresholds would not fix it and would raise false activations. Without echo cancellation, interruption would work mainly in pauses or with headphones, which does not meet the goal of interrupting while notdot speaks.

## What a supported version needs

A clean microphone signal while notdot speaks. Two practical routes:

1. **Webview capture with echo cancellation.** Replies already play in the webview. Capture with `getUserMedia({audio: {echoCancellation: true}})`, convert to 16 kHz PCM in an AudioWorklet and feed the existing local detector. WebView2 (Chromium) and WKWebView document echo cancellation for their own playback. WebKitGTK support depends on how the distribution builds WebKit and GStreamer, and is unverified. Each platform needs real speaker-and-microphone measurement before any claim.
2. **Native playback with a reference canceller.** Play replies natively and pass the exact playback signal to a mature canceller, such as the WebRTC audio processing module, before the detector. This is more portable but moves playback out of the webview and adds a native C++ dependency.

Either route then needs: the sentence guard (no interruption listening while a sentence that contains the wake phrase plays), a post-recognition check that the request is not the reply's own words, trusted-voice checks that resume the reply for an unknown or different person, and resuming the reply after silence or a refusal. These behaviours exist on a local work branch, not in this release.

## Release measures

Before shipping, measure per platform and microphone/speaker setup: detection while speaking at normal and loud playback, false activations per hour of playback (including replies that quote the wake phrase), time from the end of the wake phrase to silence, and resumption after a refusal. Synthetic results must be labelled as such and kept apart from real-person results.
