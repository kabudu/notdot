# Unattended checks — 29 October 2026

Windows microphone capture no longer stops on the reproduced buffer warning. Full Windows voice acceptance remains incomplete: the unattended speaker-to-microphone math requests did not produce the expected transcript or answer.

## Reproduction and fix

A single click with the diagnostic build reproduced the early stop and displayed the native error: “A buffer underrun or overrun occurred.” CPAL 0.18.2 reports this as `ErrorKind::Xrun`. Its WASAPI implementation reports a discontinuity and continues delivering audio; notdot was choosing to stop the recording.

The first-data-only and subsequent zero-input startup guards both failed on the actual Windows session. The Mac locked before testing the second candidate initially; it was tested after unlocking and still failed. Neither hypothesis is retained.

The final fix lets CPAL continue after its recoverable `Xrun`, `RealtimeDenied` and `DeviceChanged` notifications. Actual device loss, permission denial, invalidated streams and notdot's own bounded-queue overflow remain fatal. This does not guarantee that an operating-system discontinuity loses no samples; transcription reliability still needs acceptance with known speech.

The Windows x64 release executable was installed and its SHA-256 verified:

```text
5c349cc27aaf25794070f17e58e4d70150c43503e08f8c73577cd1c7c578f5c3
```

## Native observations after unlocking

- Capture stayed active instead of immediately opening Settings with the buffer error.
- The first attempt reached thinking, speaking and idle, with nonzero playback RMS in the bounded visual diagnostic. The companion chat contained a completed request and a clarifying response. The recognised request was unrelated background speech, not the synthetic math phrase. This is evidence of a running pipeline, not a passing accuracy or audible-reply test.
- A second synthetic request did not add the expected math turn. The cause of the synthetic playback not reaching recognition is unresolved; remote-session echo processing is a possibility, not a finding. Mac output was unmuted, built-in speakers at 56%, with the built-in microphone selected.
- A subsequent click visibly entered listening, with the Windows microphone indicator active. Escape released the microphone without opening an error; the orb showed its circular keyboard-focus cue. This verifies capture cancellation, not interruption of an active model turn or spoken reply.
- The app closed cleanly and was restarted. Saved Terra Medium, Deepgram and voice credentials were retained through the preceding updates; no credentials were re-entered during these checks.
- Windows reported reduced motion. Ordinary speaking-motion quality is not accepted from this remote session. A native menu strip was visible over the orb; global menus are now restricted to macOS, with the tray menu retained elsewhere.

A new developer example, `inspect_companion`, reads the saved chat through `thread/read` without resuming it or sending a turn. It confirmed the completed background-speech exchange on Windows and keeps notdot as the sole writer. It prints up to two recent turns and should be run only against a test chat or with permission to inspect the selected chat. It reads no voice secrets. Windows defaults use the installed native CLI and notdot settings; other systems can pass those two paths explicitly.

An earlier PowerShell read-only probe was blocked by script policy. That policy was not changed or bypassed. Antivirus protection was also retained.

## Automated checks

On the final microphone patch:

- `cargo test --workspace --locked`: 108 tests passed, 11 explicitly live/private-fixture tests ignored by this invocation.
- Formatting, diff whitespace checks and native all-target Clippy passed with warnings denied. The read-only example separately passed Clippy and a Windows release build.
- The Windows x64 release executable and unsigned NSIS bundle built successfully. SDK debug-symbol linker warnings do not indicate a missing runtime DLL.
- Regressions cover recoverable versus fatal notifications, bounded backlog, sample/callback-tail preservation and retaining errors through cancellation cleanup.

Earlier in this session, four existing ignored tests were explicitly run against the authenticated Mac Codex runtime using synthetic data and temporary memory stores: completed-interaction learning, readback-before-persistence, semantic learning/distillation and untrusted-memory injection rejection. All passed. The readback test checks the question/explicit text-approval boundary, not acoustic speech. WASM/core/voice Clippy, all 10 Python contracts and the website structure check also passed before the final native-only change. These are not Windows E2E results.

Logs and build artifacts remain under ignored `.local/windows-e2e/`.

## Remaining acceptance

Verify a known spoken request and correct audible answer on Windows, response timing, model-turn and playback interruption, repeated turns and wake activation. The remote synthetic input path currently cannot establish those results. The Mac locked again before the menu-strip fix could be installed and checked. Its Windows release build and macOS Clippy passed, but native UI acceptance of that final change is pending. The installed Windows executable is the microphone-fixed version whose hash is recorded above; a newer build is staged for the next unlocked session. No demo recording was started; no voice keys or authentication files were exported.
