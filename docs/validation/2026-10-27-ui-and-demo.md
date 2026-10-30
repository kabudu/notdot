# Settings and website check — 27 October 2026

Working-tree validation on macOS before this checkpoint:

- Local CI: 59 Rust tests passed (four service/device tests intentionally ignored), native and WASM Clippy passed including debug UI, six standalone interaction/visual tests and ten Python checks passed.
- QA app bundle built with the microphone entitlement. Native inspection confirmed nine collapsible Settings sections, combined model search and thinking-level radios, Terra/Medium reset, aligned voice icons, and no rectangular canvas focus frame. Header dragging left Settings open and retained its draft; exact desktop displacement was not measured.
- The app-menu update command opened Startup & updates and attempted a check. No live update feed is published yet, so downloading/installing/restarting an update remains unverified. The final menu also retains standard Edit commands.
- Live ElevenLabs usage refresh succeeded after the owner-approved User permission change. No API key was exported or committed.
- A real microphone/model/spoken-reply exchange succeeded on build 2a06699. The owner rejected its background noise, so the video and embeds are withheld. A new recording waits for a quiet environment; the latest speaking animation has unit coverage but still needs a fresh spoken-turn visual check.
- Website inspected at the narrow in-app browser viewport. Its orb uses a luminous core and translucent animated shells, with reduced-motion rules. Local site-reference checks passed. No stack, model or effort marketing is shown.
- Committed-history secret scan: no leaks in 33 commits. Private captures, rejected media and local tooling remain ignored.

VoiceOver, latest-build live voice, Windows/Linux hardware, sign-in startup and installed-update acceptance remain open. Repository visibility, Actions, releases and Pages publication are unchanged.
