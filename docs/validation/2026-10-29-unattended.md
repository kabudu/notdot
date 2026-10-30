# Unattended checks — 29 October 2026

Windows one-click voice and cancellation checks now pass in the remote session. Earlier unattended synthetic speech did not establish accuracy; the later human voice check did. Full platform release acceptance remains open.

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

## Follow-up after the human voice check

The menu-strip fix was installed and visually verified: Windows shows only the orb, with no native menu bar. The user confirmed that one click, a spoken request and a pause automatically submitted the request and audibly answered “Five plus seven is twelve,” without another click. Saved settings still select Terra Medium, Deepgram and ElevenLabs. The read-only probe independently confirmed the request and answer in the companion chat.

This is a passing real microphone → streaming transcription → model → spoken reply check. It is not broad accuracy acceptance: the first human attempt was misheard and elicited a clarification; the successful attempt preserved the arithmetic but still misheard “briefly.” Synthetic speaker-to-microphone failures are not reclassified as passes.

The successful turn reached thinking at `1793300651608` and speaking at `1793300653481`: **1.873 seconds from the thinking transition to first playback**. This excludes speech capture and endpoint detection and is one observation, not a latency benchmark.

### Cancellation

A temporary QA executable enabled the debug-only composer while using the existing Windows test chat and credentials. It did not create a separately isolated profile or change providers. No voice secret was printed, exported or copied. The production frontend was built separately and contains no debug composer.

- **Model interruption passed:** a request for two hundred detailed compiler paragraphs was submitted and immediately cancelled through Escape. The supported `thread/read` result reported the latest turn as `interrupted`, with the synthetic request and no completed reply. Thinking returned to idle after 623 ms.
- **Playback stop state passed:** for a fifty-paragraph compiler request, Send and a timed Escape were executed in the same UI call, avoiding orchestration delays. The app entered thinking at `1793303563237`, speaking at `1793303565359`, then left speaking at `1793303569993` and reached idle at `1793303570487`. Cancellation cut the speaking phase short after approximately 4.6 seconds. This verifies native cancellation and UI state; a human did not separately confirm the exact audible stop.
- Earlier attempts cancelled too late because separate tool calls added delay. Those attempts do not establish model interruption; only the controlled checks above are counted.

### Final build and checks

A new production build includes the missing-runtime setup fix found during Linux testing. Its unsigned NSIS bundle built successfully. The installed executable was replaced and its SHA-256 independently checked in Windows:

```text
fcf173ba0eb0dee9c571f9b134e96a3d7ca0729539d8cc70ef19954bb1a18531
```

The QA executable was closed and moved out of the installed app directory; the normal voice-only app was restarted. The final build changes runtime discovery, not voice processing. The human voice result above belongs to the preceding menu/microphone-fixed build; it was not repeated after this final rebuild.

`scripts/ci-local.sh` passed: **109 Rust tests**, 11 explicitly ignored live/private-fixture tests, native and production/debug WASM Clippy with warnings denied, formatting, all 10 Python contracts and whitespace checks. Linux-specific OS checks and emulation limits are recorded in [Linux validation](2026-10-29-linux.md). Generated binaries and logs remain ignored.

## Remaining acceptance

Windows wake activation, wider real speech/accent/noise coverage, ordinary speaking animation outside reduced-motion RDP, startup and update/restart acceptance remain open. Linux still needs a native x64 machine with a genuine runtime/account and voice services for full end-to-end acceptance. Neither cross-compilation nor emulation closes those gates.

No demo recording was started. The Mac was temporarily kept awake for testing using a bounded power assertion; password and screen-lock settings were not weakened. The temporary assertion was removed after testing; its process exited and the display-idle sleep assertion returned to zero. Linux audio used only a container null sink, so it never competed with the Windows microphone test.
