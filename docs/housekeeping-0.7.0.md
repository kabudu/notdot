# v0.7.0 acceptance plan

Implementation and validation are tracked in [the validation report](validation/2026-11-09-housekeeping.md). Automated, simulated and hardware evidence are distinguished there; unchecked hardware boundaries are not release passes.

| Area | Required behaviour | Evidence required |
| --- | --- | --- |
| Setup | Detect Codex, guide installation from an official source, reuse sign-in or offer browser login, handle cancellation and expiry, reconnect without Desktop in the foreground | Clean-profile and returning-user flows; missing runtime; cancelled and successful sign-in |
| First conversation | Guide voice configuration, microphone permission and a spoken test; save progress; separate remembering from interruptions | Setup-state regression tests and native UI checks |
| Efficiency | Bound learning prompts and repeated work, reduce unnecessary rendering, measure memory recall and startup overhead | Before/after measurements with unchanged outputs; resource and cancellation checks |
| Memory | Preserve corrections and provenance, exclude temporary/unsupported facts, bounded relevant recall, reliable forgetting and profile isolation | Repeatable multi-interaction evaluation with failure cases |
| Proactivity | Explain suggestions, suppress stale/repeated suggestions, voice snooze/dismiss, respect quiet hours, mute, profile and opt-in | Policy boundary tests and native integration checks |
| Recovery | Useful setup errors and retry; no credential exposure, polling storms or duplicate runtimes | Timeout, network and cancellation checks |
| Delivery | Update user docs, website and changelog; three-platform CI; commit and push a reviewable PR | Green checks and explicit remaining hardware gaps |

No new voice provider, automatic spending, or silent expansion of chat permissions is included. Real account authentication and OS security prompts remain user-owned.
