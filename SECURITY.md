# Security policy

## Supported versions

notdot is an unreleased development preview. Security fixes target the current default branch; there are no supported stable releases yet.

## Report privately

Do not post vulnerabilities, API keys or private transcripts in public issues. At public launch, use GitHub’s **Report a vulnerability** button in this repository’s Security tab. Private vulnerability reporting is not available on the current private repository (the configuration API returns 404); enabling and verifying that channel is a launch gate. Until then, use an existing private maintainer channel if you have one. If you do not, open an issue requesting a private security contact **without disclosing the vulnerability**. We do not promise a response deadline.

Include affected versions, prerequisites, minimal reproduction, impact and any proposed fix. Use staged data. Coordinate disclosure after a fix is available.

## Boundaries

Provider credentials use the OS keychain. Conversation and memory data are local application data, not encrypted separately by notdot; protect your OS account and backups. Activated audio goes to the configured voice service; transcripts and recalled context go to the Codex runtime/model. Wake-word listening stays on your computer until the companion wakes. A wake phrase is not authentication. Permission to remember is separate from permission to interrupt or take actions outside the chat.

The installed authenticated Codex runtime and connected providers have their own permissions and policies. The connection to Codex is experimental. Lock-screen microphone behaviour, signed installed updates and Windows hardware acceptance remain validation gates. See [current evidence](docs/validation/2026-10-27-acceptance.md). Never treat a test pass as a security guarantee.

## Latest review

See the [27 October security review](docs/validation/2026-10-27-security-review.md) for scope, fixed findings, actual-runtime probes and remaining release gates. Companion chats disable local coding/control tools and require human review for connected-tool writes; approval requests that notdot cannot present are denied. This does not make third-party tools or their read-only annotations trustworthy by itself.
