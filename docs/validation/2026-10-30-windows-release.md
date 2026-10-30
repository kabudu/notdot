# Windows release validation — 30 October 2026

Source: `678254ce38a7c5c0a2626b6f197be36d41eba83a` (0.1.0 candidate).

## Native decoder build

The original Windows helper used an implicit Visual Studio CMake path that removed release optimisation flags. The guarded helper now uses Ninja and clang-cl, verifies the effective release optimisation flags, and preserves C++ exception support. The native generic decoder is rebuilt under the selected generator too. Recognition rules, model and production deadlines were unchanged.

The synthetic pipe probe reported about 0.078 seconds to readiness and 0.906–0.922 seconds per recognition request, compared with roughly 9 seconds before the compiler fix. These are helper timings on hosted Windows runners, not complete microphone-to-reply latency.

## Intermittent 48 kHz fixture result

The [initial installer job](https://github.com/kabudu/notdot/actions/runs/41078591617/job/124141356369) rejected the positive 48 kHz fixture. The other five failures were consequences of the shared test guard becoming poisoned. No decoder error was reported, and the fallback was no longer busy. That logging does not distinguish a semantic rejection from a helper becoming unavailable and switching to generic fallback.

The independent native Windows validation job passed on the same source. One fresh installer-job attempt also passed the full Rust/model tests without source changes. This was one bounded rerun, not a change to the acceptance assertion or recognition deadline.

A [separate diagnostic](https://github.com/kabudu/notdot/pull/14) ran the 48 kHz positive fixture in 20 separate test processes: **20 passed, zero failed**. Each produced a valid keyword-model match; test bodies took 1.67–1.93 seconds, including model setup. No rejection diagnostics appeared, so this does not establish the cause of the original miss or prove that fallback transport caused it. No production fix is claimed for that observation.

The diagnostic branch is not part of the shipped application. It changes only test logging and the temporary CI probe. Keep the strict positive fixture and investigate further if the miss recurs; do not classify native crashes, decoder errors or a busy worker as an accuracy waiver.
