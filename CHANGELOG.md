# Changelog

## Unreleased

- Add opt-in automatic learning of source-linked user facts/preferences, corrections and model-generated memory digests, using isolated ephemeral Terra Medium analysis.
- Make memory-off and forgetting invalidate pending learning; expose origin/evidence, learning status and bounded-queue skips in Settings.
- Reject unresolved cancellation identities and recover timed-out submissions without sending an invalid interrupt request.

- Add opt-in local wake phrases derived from the companion name, with recommended “Hey + name” and optional name-only activation.
- Bundle sherpa-onnx keyword models/tokenisation, automatic silence stopping, bounded post-activation capture, listening/mute orb indicators, and Settings/tray microphone mute.
- Pause wake capture around voice processing, playback and announcements; discard pending audio after mute, rename, cancellation or frontend timeout.

- Add pluggable provider usage reporting, cached ElevenLabs allowance/reset details and Settings refresh.
- Announce 80/90/100% thresholds using local system speech between turns, with durable per-cycle deduplication and orb warning states.
- Distinguish credit exhaustion, authentication, permissions and temporary limits; preserve unknown/separate STT and API billing boundaries.

- Add the notdot orbit logomark to platform icons, tray and About.
- Add manual, sign-in and desktop-launch watcher startup preferences, plus optional macOS background desktop launch.
- Add six-hour update polling, persisted deferral, local spoken announcements, explicit voice confirmation, download progress and restart through Tauri's signed updater.
- Restrict update packages to the expected stable version, operating system, architecture and GitHub release path; verify version-bound signatures before installation.
- Prepare unsigned-publisher Windows NSIS and Linux AppImage/Debian packaging, with public release automation gated and GitHub Actions disabled during private development.
- Keep startup responsive with a background metadata-only check for saved macOS keys.
- Fix cancellation requests to identify the active turn, suppress the Windows console, and fit settings to the available screen.

These are development changes. Installed-platform voice/update acceptance and public distribution remain open; see docs/release-readiness.md.
