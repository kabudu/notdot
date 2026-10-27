# Security policy

## Supported versions

notdot is an unreleased development preview. Security fixes target the current default branch; there are no supported stable releases yet.

## Report privately

Do not post vulnerabilities, API keys or private transcripts in public issues. At public launch, use GitHub’s **Report a vulnerability** button in this repository’s Security tab. Private vulnerability reporting is not available on the current private repository (the configuration API returns 404); enabling and verifying that channel is a launch gate. Until then, use an existing private maintainer channel if you have one. If you do not, open an issue requesting a private security contact **without disclosing the vulnerability**. We do not promise a response deadline.

Include affected versions, prerequisites, minimal reproduction, impact and any proposed fix. Use staged data. Coordinate disclosure after a fix is available.

## Boundaries

Provider credentials use the OS keychain. Conversation and memory data are local application data, not separately encrypted by notdot; protect your OS account and backups. Activated audio goes to the configured voice service; transcripts and recalled context go to the Codex runtime/model. Optional wake detection stays local before activation. A wake phrase is not authentication. Learning consent is separate from interruption consent and external-action authority.

The installed authenticated Codex runtime and connected providers have their own permissions and policies. Runtime integration is experimental. Lock-screen microphone behaviour, signed installed updates and Windows hardware acceptance remain validation gates. See [current evidence](docs/validation/2026-10-27-acceptance.md). Never treat a test pass as a security guarantee.
