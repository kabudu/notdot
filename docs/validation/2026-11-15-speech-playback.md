# Speech playback, interruption and web boundary — 15 November

Status: branch `claude/desktop-audio-runtime-559642`, not released. These are automated checks of software behaviour. Nobody has listened to the result yet. Real-person checks are listed at the end.

## Reported problems

1. Spoken sentences sometimes overlap or override each other.
2. Playback briefly sounds about 1.5× faster, then returns to normal.
3. Gaps or breakup between sentences.
4. “Actually…” cannot interrupt without the wake phrase.
5. Asked to search a website directly, notdot said it couldn't use the browser or the computer.

## Causes found

| | Cause | Evidence |
| --- | --- | --- |
| 3 (app share) | After each sentence ended, the player waited for an 80 ms poll and then made an IPC round trip (`wake_activity`) before decoding and starting the next clip. Only one sentence was synthesized ahead, so a slow request after a short sentence left silence. | Harness, production WASM in WebKit: 68–81 ms added at every boundary where the next clip was already ready (6 of 6 runs). |
| 1, 3 | A detected interruption during the gap between sentences paused nothing, and the speech loop went on to play the next sentence while notdot was capturing the person's request. | Harness: the next sentence started while the reply was held (baseline fails “a held reply does not start its next sentence”). |
| 1–4 (with interruption on) | Interruption listening followed the media element's `paused`/`ended` state, so a poll in a sentence gap or synthesis underrun closed the echo-cancelled microphone, and the next sentence reopened it. Each reopen resets echo cancellation and can reconfigure the audio device (on Bluetooth headsets, a profile switch). Speech in a gap was missed. | Harness: two to three microphone opens per seven-sentence reply, with a close 56 ms after a sentence ended. The device effect is a hypothesis and has not been measured. |
| 4 | Voice interruption is off by default, and the owner's saved settings have it off. With it off, nothing listens during a reply, by design. Separately, an utterance ended after 600 ms of silence, so pausing after “Actually” sent only “Actually”. | Settings inspected read-only. Native unit test for the endpoint. |
| 3 (WebKit share) | After one clip ends, WebKit's media clock starts moving for the next clip about 280 ms after `play()` on the same element. This is media-pipeline start latency, not app code. It remains after this change and is probably the larger part of the audible gap. | Silent probe (`scripts/probe_webkit_playback_gap.mjs`), headless WebKit, muted, five boundaries × four runs: 280–284 ms on one element. Preloading the next clip in a second element gave 188–195 ms in most boundaries; in one run three of five boundaries stayed at about 282 ms. |
| 2 | **Not identified.** The player's timebase is correct, so the speed-up is not a `playbackRate`, sample-rate or clock fault in the media element. The remaining **hypotheses** are untested. (a) Per-sentence ElevenLabs generations without context vary in pace; ElevenLabs documents “abrupt changes in prosody” between chunks, and `previous_text` is its continuity aid. (b) The output device is reconfigured, such as a Bluetooth headset switching profile when a microphone opens or closes near playback. | Silent probe (`scripts/probe_webkit_playback_rate.mjs`, real WebKit, muted, six 44.1 kHz mono MP3 sentences in ElevenLabs' default format) found: `playbackRate` and `defaultPlaybackRate` stay 1, `preservesPitch` stays true, and decoded and declared rates are both 44,100 Hz. Decoded duration equals element duration. The media clock never exceeded 1.005× in any 200 ms window. Real ElevenLabs output and per-chunk speaking rate were not measured, because that needs paid synthesis. |
| 5 | notdot's Codex runtime turns off `browser_use` and `computer_use`. The owner's Codex desktop configuration does include browser and computer-use plugins and a `node_repl` MCP server wired to the ChatGPT app's browser backends; its `computer-use` MCP entry is disabled. These are host features of the Codex desktop app, and app-server clients have no documented way to provide them. Its web search follows the person's Codex `web_search` setting. The default, `cached`, searches an index and cannot open a site's current page. Trusted-voice profiles turn search off. The model was given no description of this boundary. | Code (`runtime_policy::companion`, `restrict_profile_tools`), official Codex configuration docs, and the installed runtime (`codex-cli 0.169.0-alpha.11.1`): `config/read` reports no `web_search` (so `cached`) and no requirements. No model turn was run. |

## Changes

- Up to two sentences are synthesized ahead through an ordered, bounded pipeline (`speech_pipeline.rs`). Results that complete out of order still play in request order. Late results after cancellation are discarded and their blob URLs released.
- Clips are decoded when synthesis finishes. The next clip starts from the element's `ended` event, and the wake lease call no longer sits between sentences. One media element is used throughout, with no Web Audio routing on macOS or Linux, so the v0.8.0 direct-output fix is kept.
- A held reply never starts a sentence. After a resume, the reply carries on from the next sentence.
- With interruption on, listening opens once per reply, covers the gaps, and closes as soon as the reply ends.
- After less than 800 ms of speech, an interruption waits for 1.2 s of silence before ending (600 ms otherwise).
- The previous sentence, at most 300 characters, is sent as ElevenLabs `previous_text`. ElevenLabs does not document whether this context is billed.
- The companion is told its actual web boundary for its search mode. No capability or permission was enabled.
- Settings now says interruption is off by default and that, while it is on, the microphone stays on during every reply, with only speech after a pause going to the recognition service. While a reply plays with interruption off, the orb's tooltip and accessible label say how to turn it on. The setting is never changed automatically. Interruption first appeared in v0.8.0, off by default, and no migration or onboarding step turns it on. The v0.8.0 changelog headline (“Say “Stop”, “Actually…”… without repeating the wake phrase”) is the likely source of the expectation.

## Automated evidence

Scenario `#speech-stream` in `scripts/fixtures/conversation-harness.js` runs the production WASM UI with synthetic IPC, model stream, synthesis, media element and echo-cancelled microphone. It plays no sound and opens no microphone. Clip timings are element-level, not decoded audio samples.

It covers:

- a seven-sentence streamed reply with synthesis delays from 50 ms to 3 s and clip lengths from 300 ms to 1.1 s;
- rapid successive turns;
- cancellation while thinking, while synthesizing, after synthesis but before playback, during a clip, in an underrun gap, and during the final clip;
- an “Actually, …” replacement;
- speech detected in a gap.

| WebKit (Playwright 1.64, macOS) | v0.8.0 (`aa5b07d`) | This branch |
| --- | --- | --- |
| Added gap per boundary, ms (all six) | 81, 74, 1, 8, 1, 73 | 0–2 |
| Interruption microphone opens per reply | 2 | 1 |
| Clips playing at once (max) | 1 | 1 |
| Sentences in order, once each | yes | yes |
| Held reply starts next sentence | **yes** | no |
| Old sentence after Escape or replacement | none | none |
| Checks passed (before the reply-hint checks were added) | 20 of 24 | 24 of 24 (now 27 of 27) |

All existing harness scenarios pass on both builds: the default 67 checks, interruption, unsupported echo cancellation, start failure, profile and tasks. The speech scenario and the default and interruption scenarios also pass in Chromium with a Windows user agent, which exercises the metered output path.

Reproduce:

```sh
trunk build --dist .local/continuity-ui
python3 scripts/prepare_conversation_harness.py
NODE_PATH=<a Playwright 1.64 install>/node_modules node scripts/run_conversation_harness.mjs .local/continuity-ui speech-stream
```

Native tests cover:

- pipeline ordering and bounds;
- the `previous_text` request body, checked against a local HTTP server;
- the lead-in endpoint;
- the web-boundary instructions for each search mode;
- `browser_use` and `computer_use` staying off.

## Follow-ups

- **Remaining WebKit gap of about 0.2–0.3 s per sentence.** Sample-accurate, gapless playback would mean decoding sentences into `AudioBuffer`s and scheduling them back to back on one `AudioContext`. That changes the output path v0.8.0 stabilised (Linux WebKit drops 15–40 ms after silence) and needs acoustic validation on each platform. A second preloaded element saves about 90 ms on most boundaries.
- **`node_repl` MCP inheritance.** Single-person companions inherit the person's MCP servers with write approval required. The `node_repl` server can run code, and its write calls are refused because notdot rejects host approval requests. Disabling it explicitly, as trusted-voice profiles already do, would be clearer. That is a product decision.

## Not established — needs a person

- The cause of the pace jumps (problem 2). Listen to several long replies with ElevenLabs, noting the output device. If jumps continue with `previous_text`, try wired speakers to test the device hypothesis.
- Real inter-sentence gaps with decoded MP3 on the owner's Mac and output device, including Bluetooth.
- With interruption turned on: real “Actually, …” overlap, echo rejection and no self-interruption on the owner's speakers.
- A real Codex turn asking for a website search, to hear the new explanation. This was not run because it uses the owner's account.
