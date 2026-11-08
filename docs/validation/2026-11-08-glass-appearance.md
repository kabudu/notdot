# Glass appearance validation

The v0.5.0 appearance changes use the production Rust/WASM renderer and the existing voice state and audio meter. Service, capture and task behaviour is unchanged by this change.

## Rendering and interaction

- The orb has a translucent body, curved surface light, star points and a soft outer shell. Listening opens one shell, follow-up listening shows two, thinking adds a travelling arc and speech gently moves the light and shell using the existing smoothed audio level. Errors blend towards a warm colour.
- Canvas work is bounded to eight 97-point ribbons, 42 stars and whole-path strokes. Rendering retains the existing 30 fps limit, 2× pixel-density cap and hidden-window pause. The animation clock clamps long frame gaps to avoid jumps after a window is restored.
- The existing saved-hue readiness gate is preserved, so the orb is hidden until preferences load. Preview colours use the same renderer. Keyboard focus remains circular and appears only during keyboard navigation.
- Reduced motion freezes surface geometry and rotation at their current position. Conversation state changes still update the activity cue and accessible status.

## Checks

- Native orb tests pass in debug and release builds. Tests cover bounded state transitions, reduced-motion pause, speech attack/release, keyboard input, capture timing, follow-up state and cancellation.
- Native and WASM Clippy pass with warnings denied, including the WASM debug UI feature. Formatting and diff checks pass.
- The actual release-mode WASM UI passed all 65 integration checks in Chrome with the existing synthetic service/audio adapters. The harness exercises enrolment, confirmation, profile removal, playback cancellation, follow-ups, task reports and spoken settings. Its final run made 11 simulated model requests and 9 follow-up captures, with a minimum playback-to-capture gap of 680.6 ms. No microphone, credentials or external services are used by this harness.
- With a test adapter reporting reduced motion, two canvas images taken 1.2 seconds apart were identical. Keyboard Tab showed the circular focus cue without a rectangle around the canvas.
- With the first snapshot delayed by 1.2 seconds and a saved hue of 285, the canvas stayed hidden before preferences arrived and the first rendered gradient used hue 285. The page reported no browser errors.
- Actual WASM screenshots were reviewed for idle, thinking and error rendering. A separate local preview exercises all six visual states and a simulated audio level.
- A diagnostic wrapper around the animation callback measured roughly 0.6–1.1 ms median and 1.1–2.1 ms p95 over windows of 240 rendered frames on the development Mac in Chrome at desktop size. These are CPU callback observations under concurrent development load, not GPU timing or a cross-platform benchmark.

## Settings accessibility

- The panel uses a theme-tinted translucent backdrop, opaque text and solid form fields. The default surface is solid when backdrop filtering is unavailable. Reduced-transparency, increased-contrast and forced-colour fallbacks are included.
- Calculated worst-case contrast across all 361 hue values is 8.87:1 for main text, 5.99:1 for secondary text and 3.46:1 for input borders, including a white desktop and the brightest glass highlight.
- The actual WASM panel was reviewed at 360×360 and 450×680. A compact-height layout leaves the content scrollable and Close/Save visible. Keyboard focus scrolls fields above the footer. The expanded Voice section has no horizontal overflow. Categories, arrow-key navigation, search, focus and live theme preview were exercised.

## Limits

The browser harness uses real WASM with simulated native IPC and audio. Packaged native webview appearance, real audio playback, screen-reader operation and OS accessibility media settings still need platform acceptance. The fallback CSS was source-reviewed; forced colours and reduced transparency were not enabled at OS level during this check. CSS backdrop filtering can blur content within the webview; desktop blur depends on the platform compositor. The package version is left for the v0.5.0 release preparation.

### Native macOS WebKit acceptance

The compiled production WASM UI also ran in a temporary, nonpersistent native
`WKWebView` host at 450 × 680 CSS pixels. This uses the same browser engine as
macOS Tauri, with a synthetic IPC adapter and no microphone, provider requests,
Keychain access or personal app data. The glass panel, clear text and control
surfaces rendered correctly. The native accessibility tree exposed the Settings
heading, labelled search field, category tabs, named inputs, switches, radio
choices, voice picker and preview button. Searching for “quiet” returned the two
matching settings sections while retaining Close and Save. This verifies native
WebKit rendering and accessibility exposure, not a VoiceOver listening session
or the packaged Tauri window's operating-system behaviour.
