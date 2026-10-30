# Contributing to notdot

Thanks for helping make a quieter, more useful desktop companion.

## Start small

For a bug, open an issue with your OS, notdot/runtime versions, reproduction steps and expected behaviour. Redact account names, transcripts, keys and local paths. For a substantial feature, discuss the user need in an issue before implementation. Security reports belong in [SECURITY.md](SECURITY.md).

## Develop and verify

Start with the [Settings guide](docs/settings.md) to understand the app, then follow the [development guide](docs/guide.md). Create a focused branch, keep changes reviewable, and explain the problem and resulting behaviour in your pull request. Run `scripts/ci-local.sh` for application changes. For documentation-only changes, check links and examples; for the website, run `python3 scripts/check_site.py` and inspect narrow and wide layouts.

Voice changes need real microphone → transcription → response → audible playback evidence, including cancellation and failure handling. Clearly distinguish mocks, synthetic fixtures and hardware testing. Never commit credentials, personal recordings or private conversation history. Use your own accounts and staged, non-sensitive demo content.

Preserve the voice-only orb interface, keyboard access, reduced-motion support, provider abstraction and explicit limits on what the app may do. Add meaningful regression coverage for changed behaviour. Include platform limitations and untested paths in the PR.

## Licensing and conduct

By intentionally submitting a contribution, you agree it is licensed under Apache-2.0, unless explicitly agreed otherwise. Only contribute work you are entitled to share; retain third-party attribution. No CLA is required. See [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

Maintainers review contributions as availability permits; there is no guaranteed response time. Current development is a preview, with release gates tracked in [release readiness](docs/release-readiness.md).

Local validation uses Python 3.12 or later for safe release-archive checks. `scripts/ci-local.sh` uses `python3.12` when the default Python is older. You can select another compatible interpreter with `NOTDOT_TEST_PYTHON`.
