# Unattended checks — 29 October 2026

Windows voice acceptance remains incomplete. The Mac locked during testing, which prevents Computer Use from operating the Windows App session. No unlock or security bypass was attempted.

## Windows reproduction

The session was accessible at the start. The diagnostic executable from `2c5233b` was copied through the existing read-only RDP share, its SHA-256 verified, and installed in place. Settings and saved credentials were retained.

A single click reproduced the early stop. Settings now displayed the actual native error automatically: “A buffer underrun or overrun occurred.” CPAL 0.18.2 reports this as `ErrorKind::Xrun`; it is not evidence that the microphone physically disconnected. The previously ambiguous callback diagnosis is narrowed to this buffer-discontinuity notification. The error-reporting and sticky-footer changes are verified through the Windows UI.

## Candidate and verification limit

The capture callback now separates advisory realtime-scheduling notices from fatal device failures. A discontinuity before any non-silent mono input is received is treated as startup; after a signal arrives, it still rejects the recording rather than silently dropping words. Permission denial, device loss and queue overflow remain failures. No retry, provider switch, extra STT pass or model change was added.

The first attempt, tolerating notifications only before the first data callback, did not solve the Windows reproduction. The revised zero-input startup condition is a hypothesis to test, not a confirmed explanation of all remote-audio interruptions.

The revised Windows x64 release executable was installed and its hash verified:

```text
1ea02ea0dcbd4d78cef4c96f6af3c1d1781fe0e126424b313af39f2f57dad93a
```

The Mac then locked before a capture attempt with that candidate. Its live behaviour is unverified. Do not count the Windows microphone issue, one-click voice turn, wake mode, spoken output or cancellation as passed. The saved reply-service key and voice selection survived the preceding app updates: native capture readiness checks passed without re-entering credentials. Terra Medium and Deepgram remained selected in Settings.

## Offline checks

On the working tree based on `2c5233b` plus the startup-notification patch:

- `cargo test --workspace --locked`: 109 tests passed; 11 explicit live/private-fixture tests remained ignored by that invocation.
- Four existing ignored tests were then explicitly run against the authenticated Mac Codex runtime, using only synthetic data and temporary memory stores: native completed-interaction learning, readback-before-persistence, semantic learning/distillation, and untrusted-memory injection rejection. All four passed.
- The readback test exercises the proposal, readback-request and explicit text-approval boundaries; it does not synthesize or recognise speech. These are Mac runtime integration checks, not Windows E2E results.
- Native/core/voice Clippy across all targets, WASM Clippy with and without the debug UI, and formatting passed with warnings denied where applicable.
- All 10 Python contracts passed, including static accessibility and release/vendor checks. The website structure checker validated 16 references; this is not browser or audio acceptance.
- The Windows x64 release build and unsigned NSIS bundle completed. Antivirus protection was not disabled or bypassed.

The new regressions cover advisory versus fatal notification handling and zero-filled startup packets. Existing tests also cover bounded input backlog, sample preservation and error retention through cancellation cleanup. Logs and build artifacts remain under ignored `.local/windows-e2e/`.

## Resume native testing

Once the Mac is unlocked, check the installed candidate with an empty capture first, then synthetic speaker-to-microphone speech. Verify the transcript and Terra Medium reply, automatic silence submission, thinking/speaking transitions, cancellation and saved settings after restart. If a discontinuity still ends capture, inspect the signal/startup boundary before changing the policy again.

No demo recording was started. Production memories and credentials were not exported or edited.
