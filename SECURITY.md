# Security policy

## Supported versions

Security fixes target the latest release and the current default branch.

## Report privately

Use GitHub’s [Report a vulnerability](https://github.com/kabudu/notdot/security/advisories/new) form in this repository’s Security tab. Private reporting is enabled. Keep vulnerabilities, API keys and private transcripts out of public issues. We do not promise a response deadline.

Include affected versions, prerequisites, minimal reproduction, impact and any proposed fix. Use staged data. Coordinate disclosure after a fix is available.

## Boundaries

Provider credentials use the OS keychain. Conversation and memory data are local application data, not encrypted separately by notdot; protect your OS account and backups. Activated audio goes to the configured recognition service (which may differ from the playback service); transcripts and recalled context go to the Codex runtime/model. Wake-word listening stays on your computer until the companion wakes. A wake phrase is not authentication. Permission to remember is separate from permission to interrupt or take actions outside the chat.

The installed authenticated Codex runtime and connected providers have their own permissions and policies. See [platform validation and its limits](release-readiness.md) for the environments and workflows exercised. Never treat a test pass as a security guarantee.

## Latest review

See the [27 October security review](docs/validation/2026-10-27-security-review.md) for scope, fixed findings, actual-runtime probes and remaining release gates. Companion chats disable local coding/control tools and require human review for connected-tool writes; approval requests that notdot cannot present are denied. This does not make third-party tools or their read-only annotations trustworthy by itself.

Deepgram Flux recognition is English-only and uses a separate native keychain entry. Speech recognition and spoken replies can use different providers. No premium hints or second transcription pass is enabled.
