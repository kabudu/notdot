# Orb and keyboard follow-up — 27 October 2026

## Verified

- Native QA inspection on the updated macOS bundle: Tab focuses the orb and shows a soft circular cue. Closing Settings by pointer leaves focus on the orb without that cue. Neither path draws a rectangular frame. Private comparison screenshots stay outside Git.
- Found a regression in the custom app menu: Quit lacked its Command–Q accelerator. Restored CmdOrCtrl+Q. Native verification passed: Command–Q exited the QA process; the same bundle reopened successfully.
- Replaced the website’s opaque CSS orb with a transparent SVG based on the app renderer’s proportions, radial gradients, highlight and 70 golden-angle star points. Gentle shell breathing and continuous star drift respect reduced motion. No JavaScript, external fonts or remote assets were added.
- Browser checks at 375 and 1280 CSS pixels found no horizontal overflow; the artwork loaded and fit the layout. Viewport overrides were reset afterward.
- Local CI passed: 59 Rust tests, six standalone interaction/animation tests, ten Python checks, native and WASM Clippy including debug UI. Site structure and 16 references passed. An initial concurrent build removed a dependency artifact during rustdoc; the sequential rerun passed.
- The ad-hoc QA app built successfully with its microphone entitlement. No notarisation or public release was performed.

## Real playback check

After the owner approved macOS saved-key access, the saved key became ready and usage refreshed successfully. Three complete voice previews followed thinking → speaking → idle; renderer diagnostics confirmed metered playback and a peak speech RMS of 0.2573. A short series of native screenshots during playback showed two broad, closed outer shells without radial spikes. The envelope and shell continuity tests also passed.

A fourth preview reached Speaking and was cancelled with Escape. The status returned to Ready and playback metering disconnected. Returning to idle after natural completion also passed. These were actual ElevenLabs preview clips through the app’s normal audio/renderer path, not synthetic mode overrides. No microphone or demo recording ran.

The completed previews lasted about 4.3 seconds. Diagnostic frame counts averaged roughly 29 frames/second during those short samples, consistent with the renderer’s 30 fps cap. This is a rough local observation, not a frame-time or end-to-end voice-latency benchmark. Private screenshots and diagnostic snapshots remain outside Git.

The promotional demo remains withheld until a quiet recording session. A fresh full microphone-to-model-to-reply test, native reduced-motion/VoiceOver acceptance and the other platform/release gates remain open. The tested QA bundle is running; production settings and memory were not changed.
