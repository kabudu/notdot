# Orb and keyboard follow-up — 27 October 2026

## Verified

- Native QA inspection on the updated macOS bundle: Tab focuses the orb and shows a soft circular cue. Closing Settings by pointer leaves focus on the orb without that cue. Neither path draws a rectangular frame. Private comparison screenshots stay outside Git.
- Found a regression in the custom app menu: Quit lacked its Command–Q accelerator. Restored CmdOrCtrl+Q. A native shortcut check is pending while the voice-key approval prompt is open.
- Replaced the website’s opaque CSS orb with a transparent SVG based on the app renderer’s proportions, radial gradients, highlight and 70 golden-angle star points. Gentle shell breathing and continuous star drift respect reduced motion. No JavaScript, external fonts or remote assets were added.
- Browser checks at 375 and 1280 CSS pixels found no horizontal overflow; the artwork loaded and fit the layout. Viewport overrides were reset afterward.
- Local CI passed: 59 Rust tests, six standalone interaction/animation tests, ten Python checks, native and WASM Clippy including debug UI. Site structure and 16 references passed. An initial concurrent build removed a dependency artifact during rustdoc; the sequential rerun passed.
- The ad-hoc QA app built successfully with its microphone entitlement. No notarisation or public release was performed.

## Waiting for local approval

Real voice preview correctly requires saved-key authorisation for the rebuilt ad-hoc app. The OS request is open. Playback/animation inspection will resume after the owner approves it; no microphone or demo recording is running.

The promotional demo remains withheld until a quiet recording session. Latest full microphone-to-reply acceptance and the other platform/release gates remain as documented previously.
