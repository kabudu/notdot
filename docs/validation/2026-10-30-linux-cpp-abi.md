# Linux x64 native C++ ABI regression

The public Ubuntu 22.04 x64 release job aborted during the bundled wake-model tests. The preceding direct-address test was unrelated: the crash was in the next test's native Whisper tokenizer.

## Cause and fix

A native GDB run reproduced `SIGABRT` at `free()`, through the modern GNU `std::string` destructor and `std::__detail::_Scanner<char>` destructor, inside Whisper tokenization. The pinned x64 sherpa-onnx/ONNX archives use the original GNU string ABI; locally built whisper.cpp used the modern ABI. Both export weak regex-scanner symbols whose names do not distinguish the incompatible embedded string layouts.

A separate local link experiment used unchanged Sherpa and the same small C++ regex shim: the modern-ABI shim failed, while compiling it with `_GLIBCXX_USE_CXX11_ABI=0` passed regex initialization and real keyword-model create/drop. This supplementary experiment ran under Rosetta, rather than establishing native performance.

`.cargo/config.toml` now applies that compiler definition only to GNU/Linux x64. Inspection of the pinned ARM archive found modern `std::__cxx11` symbols, so ARM is unchanged. Apple and MSVC are also unchanged. The GNU compiler's [dual-ABI documentation](https://gcc.gnu.org/onlinedocs/libstdc++/manual/using_dual_abi.html) explains the differing string layouts and the possibility of identical names for classes containing strings.

The pinned native build script does not track changed C++ flags. Existing developer caches need `cargo clean -p whisper-rs-sys` once; Linux CI explicitly rebuilds that package before testing. The generated CMake cache was checked for the actual ABI definition, rather than assuming the Cargo setting took effect.

## Native before/after evidence

- [Before, c14dbde](https://github.com/kabudu/notdot/actions/runs/41053587770/job/124055420966): unchanged native x64 suite under GDB reproduced the allocator abort in `bundled_phrases_reject_name_only_unrelated_audio_and_cancelled_work`.
- [After, 8ab97a3](https://github.com/kabudu/notdot/actions/runs/41054324252/job/124057901316): the same real unit suite under GDB exited normally: **57 passed, 0 failed, 11 ignored**, in 63.21 seconds.
- Passing coverage includes real bundled fallback decoding, direct-address rejection, custom-name/keyword decoding, 48 kHz input, reset/cancellation, update-release ownership and extended-idle fixtures. No thresholds, decode deadlines or assertions were relaxed.
- The ignored tests require explicit live microphone, keyring, provider or private-fixture access; this investigation did not run them. No UI, microphone, playback, provider requests or credentials were used.

Raw downloaded before/after artifacts are retained locally under `.local/linux-ci-before/` and `.local/linux-ci-after/`. The normal `linux-native` CI job retains real-model regression coverage; temporary GDB instrumentation was removed.

The supplementary emulated x64 full run no longer aborted after the ABI correction, but its fast detector-reuse fixture missed a wake and poisoned the fixture mutex (52 passed, 5 failed, 11 ignored). This was not reproduced in the genuine native x64 job above; it is not reported as a passing emulated suite.
