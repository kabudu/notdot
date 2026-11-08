# Glass appearance validation

The v0.5.0 appearance changes use the production Rust/WASM renderer and the existing voice state and audio meter. Service, capture and task behaviour is unchanged by this change.

## Updated dark-core shader integration

The initial Canvas appearance below is superseded by the Rust/WASM WebGL renderer in this PR. The core is almost opaque, with translucent perimeter light. Speech drives stronger wave brightness, a 7.5% maximum body expansion and an outward, brighter shell; thinking retains a travelling arc. Geometry remains frozen under reduced motion. The original accessible Canvas remains above the shader for keyboard focus and quota, mute and wake indicators, and provides a dark-core fallback if WebGL cannot start or loses its context. A failed restoration stays on Canvas rather than retrying compilation every frame.

`python3 scripts/prepare_shader_harness.py` wraps a fresh release Trunk build in reproducible state, theme, error, context-loss and recovery checks. Native window click-through and packaged-platform acceptance remain required before merge. Earlier JavaScript prototype benchmark numbers are not measurements of this integrated renderer.

## Initial Canvas rendering and interaction

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


### Full settings acceptance

The reusable settings harness runs the production WASM UI with synthetic
provider, model, usage and people data. It passed 122 checks at each of
320 × 360, 360 × 360, 450 × 680 and 900 × 900 CSS pixels in Chrome. Every
category and all 11 main sections are expanded and their controls focused.
Checks include horizontal overflow, focus scrolling, visible footer actions,
three model cards and five effort levels, eight searchable voices, aligned
voice and preview icons, usage progress labels, empty searches and clear,
seven theme colours, About modal return, all four enrolment phrases and named
member-removal cancellation. These are layout and UI-flow checks, not speaker
recognition or provider acceptance.

This exposed and fixed overflowing long member-action labels, focus remaining
outside enrolment, lost focus after enrolment/removal cancellation and the
final enrolment phrase, and long errors displacing the settings content.
Long errors now scroll in a bounded, keyboard-focusable status area.

The final compiled UI also passed the existing 65-check integration suite
again (11 simulated model messages, 9 follow-up captures, minimum playback-to-
capture gap 660.4 ms). Native debug/release tests, native/WASM Clippy with
warnings denied, formatting, Python compilation and JS syntax checks passed.

The same 122 checks also passed in the native macOS WKWebView host using its
public 200% page zoom setting: a 900 × 900 host produces a 450 × 450 CSS-pixel
viewport. Native accessibility inspection separately verified that enrolment
focuses consent and Cancel returns to Teach voice. This is accessibility-tree
inspection, not a claim of a completed VoiceOver listening session. OS-level
forced-colour and reduced-transparency preferences remain source-reviewed;
they were not changed on the user's Mac. Packaged Windows/Linux webviews and
real voice interaction remain outside this appearance-only acceptance run.

To reproduce the settings matrix without keys or audio:

```sh
trunk build --release --dist .local/settings-qa
python3 scripts/prepare_settings_harness.py
python3 -m http.server 8101 --bind 127.0.0.1 --directory .local/settings-qa
```

Open `http://localhost:8101/runner.html` and select **Run all layouts**.
Screenshot evidence and the native JSON report were retained locally under
`.local/settings-evidence/`; they contain synthetic profiles only.

## Native input layer and branding

The collapsed visual window is wholly click-through. A separate, smaller input window sits inside the core; the remaining decoration does not intercept pointers. Opening Settings hides the small window and makes the main window interactive. Layout follows window movement and display scaling without pointer polling. The centre stays hidden until the saved appearance is ready. A layout failure restores the original main-window interaction. Keyboard actions and state labels pass through the same WASM controls.

The isolated `orb-input-acceptance` example registers only input layout. Its UI adapter simulates services, profiles and audio, so it cannot access saved keys or initiate a real model/voice request. Native Mac checks have exercised centre activation, Escape cancellation, Settings opening and return to the centre. Full drag and outer click-through acceptance, Windows native input acceptance and Linux native input acceptance remain merge gates. Engine-level shader lifecycle checks passed on macOS WebKit, Windows Edge and Ubuntu ARM64 WebKitGTK; those are distinct from native OS-window tests.

The app icon, About image, README mark, site header and favicon use the dark-core identity. `scripts/sync_brand_assets.py --icons` synchronises the shared artwork and rebuilds the desktop formats. The website uses the app fragment shader with a larger illustration footprint, a reduced-motion/off-screen pause and an SVG fallback. The site check allows only that one deferred local script and rejects inline or third-party scripts. The old demo remains unchanged until a fresh real-app recording is made; it is not a recording of the new appearance.

CI now prefetches each pinned Sherpa native archive once per job, verifies the publisher's SHA-256 and exact byte size, and shares that archive across Cargo profiles. This addresses observed HTTP 503/504 failures in later profile builds without changing native libraries or skipping checks. The universal Mac path verifies both architectures. Corrupt or unexpected archives fail before Cargo can reuse them.

Native input follow-up on 8 November: macOS passed core click, Escape cancellation, keyboard activation, right-click Settings, returning from Settings, and dragging without capture. Moving the main window by pointer displacement also moved its child input target. Linux/Xfce initially expanded the input target to WebKit's 200px natural minimum; allowing GTK resize with explicit minimum/maximum constraints kept the actual target at 84px after mapping. Core click and Escape passed in the VM, and clicking the halo selected the underlying Thunar file without capturing speech. VM viewer drag and right-click attempts delivered pointer moves but no corresponding pointerdown event in the guest; those gestures are not claimed as native passes from that transport. The credential-free native fixture uses production WASM and window code, with synthetic voice/model services, so these are input/graphics checks rather than real voice results.

The refreshed production WASM shader harness passed all 13 lifecycle checks on macOS WKWebView, Windows Edge over RDP and Linux WebKitGTK after the input fixes (source e4dcab6; later 809bf94 adds only CI regression coverage). These exercise themes, listening/thinking/speaking state uniforms, error tint, actual context loss, Canvas fallback and successful restoration. Edge results are engine checks, not a substitute for the native WebView2 input host.

The Developer ID-signed Mac candidate reused the existing saved Deepgram and ElevenLabs keys without a new prompt. An unattended speaker-to-microphone check reached wake listening, speech capture, thinking, metered playback and follow-up. A second check completed two spoken turns without another wake phrase and stopped after “That's all”: idle returned 3.03 seconds after the follow-up opened, before its 8-second deadline. A separate silent follow-up closed after 8.039 seconds. Playback telemetry measured 30.12 animation frames/second and a nonzero RMS peak of 0.384. These are generated-speech and application-state observations; a person has not yet judged the new demo's recorded sound or speaking appearance. No demo recorder ran during these checks.

Windows laptop input-host attempt: CI run 42476711657 built `orb-input-acceptance.exe` from e4dcab6. SHA-256 `5c22c3e0b00f64aec6ab68eb1a031cdc24f4cf3f6cc42b10d4145055384e0367` matched before launch. Avast scanned the executable and Windows returned Access denied; retrying the same bytes after the scan initiated another scan. Protection was not disabled and no exclusion was added. Native Windows hit-routing/gesture checks therefore remain unverified on this laptop. Windows CI native tests, Clippy and NSIS packaging passed on that source revision; those results do not replace native desktop gesture evidence.

After the first real v0.5 demo take, the user reported that wake acknowledgement was visible mainly in faint outer rings. Listening now adds a single fading bloom inside 94% of the opaque core, then a gentle sustained listening glow; reduced motion uses a constant glow. This shared Canvas overlay applies over both WebGL and fallback drawing and follows the saved theme. All 27 frontend unit tests passed, including cue entry, no retrigger during capture, reset and reduced motion. Updated graphics harness checks passed 15/15 in macOS WKWebView, Windows Edge and Linux WebKitGTK, including reading actual overlay pixels during listening and checking that they clear after capture. These engine checks retain the native Windows gesture gap described above.

The second real demo take confirmed the inner wake bloom with the user and completed both replies plus the spoken stop. The user still heard a speaker pop at reply starts. The existing 25ms gain envelope is therefore not accepted as proof that this issue is fixed. A further candidate reuses the same media element and Web Audio output graph across replies, clearing the source and decoded audio on cancellation/profile changes while retaining only the empty element and nodes. Linux keeps its existing plain HTML audio path. This candidate passed 27 frontend unit tests, WASM Clippy, and all 67 synthetic integration checks, including two new checks for graph reuse and speech-source clearing; the synthetic echo gap remained at least 700ms. The signed Mac candidate also completed an unattended two-turn microphone/model/playback/follow-up/stop cycle. Acoustic acceptance is pending the user's listening result, so the pop is not yet marked resolved and the demo has not been published.

On 8 November, the user confirmed no pop in either reply from the signed output-reuse candidate. This establishes the Mac built-in-speaker listening result for those two turns; other audio devices are not covered by that observation. The recorded demo take predates this output change, so its captured clicks are reduced during export rather than being presented as a recording of the later binary. Both raw takes remain local. After recording, the recorder microphone switch and screen/system-audio switch were verified off in System Settings.

CI run 42480527788 passed the Linux release-mode native suite but its separate 48kHz advisory ran the unoptimised debug binary and hit the existing recognition time limit. The advisory command now uses `--release`, matching the installed runtime and the rest of the native model suite. The timeout, exact-test classifier and native-crash rejection are unchanged. CI run 42481763058 subsequently passed both the Linux native suite and the release-mode advisory without changing the policy.

The final demo export is 30.3 seconds at recorded speed, with both real replies, a follow-up without a new wake phrase, and the spoken stop. Only the lead-in and ending were trimmed. Export crops the captured orb window for presentation, balances microphone/reply levels, uses a 70Hz high-pass and conservative click removal, and measures -18.1 LUFS with -1.48dBTP after encoding. Six captions, the original transcript, current icon and the required ElevenLabs attribution accompany it. Chrome playback and visible selectable English captions passed with the production `text/vtt` content type; the local default Python preview needed that MIME mapping. The uploaded README asset and the website captioned MP4 have the same bytes.

The output-reuse candidate also passed the two isolated profile-capture assertions and the profile-playback assertion in the real WASM/synthetic-service harness: a speaker identification preserves the active capture and submits it once, while a profile switch stops old playback and drops queued sentences.

## Native input follow-up, 8 November

A localhost-only noVNC connection to the same Ubuntu ARM64 desktop forwarded the button events missing from the Virtualization.framework viewer. It reproduced and then verified two fixes through the actual GTK/WebKit input host:

- A coalesced drag stopped at the last move rather than the release point. The input control now applies release coordinates too. The regression assertion failed on the previous script and passed with the fix; native dragging reached the intended endpoint without starting a capture.
- After click-through was disabled for Settings, lower panel controls did not receive clicks. Tao 0.37.2 applies its ignore shape to GDK but restores the GTK shape. Explicitly updating the GDK input region on the GTK main thread restored the Close button. The production input module then passed right-click opening, lower-button closing, centre activation, Escape cancellation and halo click-through to Thunar after closing. Queued shape changes check the current collapsed state before applying. No saved keys, microphone or provider calls were used.

Windows accepted the original hash-verified helper on the later retry and exposed a missing Common Controls v6 manifest (`TaskDialogIndirect` loader error). Commit c4710ac embeds that dependency for Cargo examples; CI run 42486195086 passed all jobs, including the new startup assertion that requires native window creation. The corrected CI artifact, SHA-256 `d20eea7e8ee1360ceea4398b58f5e443f8ae246fbe6ccac02355bb79ef401e2c`, also created its windows on the laptop. Avast then blocked it pending Threat Labs analysis, explicitly estimating a few hours. No protection or exclusions were changed. Native laptop gesture checks remain unverified; loader startup is now verified separately.
