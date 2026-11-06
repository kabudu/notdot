# Release measures

We judge releases by whether people can speak naturally and finish useful work. These are initial acceptance targets, not claims about measured performance. Record the baseline before tightening them.

| Measure | How to measure it | Initial target |
| --- | --- | --- |
| Wake success | Correct activation after the configured phrase; include normal distance, soft speech and idle recovery. | At least 95% of 40 attempts per platform |
| False wake-ups | Activations during background speech and silence, excluding the wake phrase. | At most 1 per hour over at least 2 hours per platform |
| Wake feedback | End of wake phrase to visible listening cue, including detector delay. | 95th percentile at most 1 second |
| First spoken reply | End of request to first audible reply, including silence detection, recognition, model and voice service. Use short questions with Terra Medium. | 95th percentile at most 6 seconds |
| Cancellation | Escape to silence and no resumed playback, follow-up capture or unintended action. | 100% of 10 attempts; 95th percentile at most 500 ms |
| Voice task completion | Correct destination, exact instruction, required permission, correct result notice, once only. Include failure and interruption. | At least 95% of 20 scenarios; no unauthorised actions |
| Conversation safety | Silence timeout, spoken stop, self-echo, muted microphone and settings-open cases. | All 10 scenarios pass; no self-triggered requests |

## Record the conditions

Keep one report for each operating system, hardware/microphone, app revision and voice-provider combination. Record whether the speech is human or generated. Do not pool different platforms or providers to hide a poor result. Report failures and timeouts alongside timings; never remove them from the success denominator. Percentiles use nearest rank.

Use the same short questions and voice-task scenarios when comparing releases. Test quiet rooms, ordinary background noise, similar-sounding words, pauses, cancellation during generation and playback, and wake listening after idle. Generated clips support repeatable regression checks; they do not establish reliability for real speakers and accents.

Reports contain counts, milliseconds and environment labels only. Do not include recordings, transcripts, chat names, API keys or account identifiers. Keep private acoustic evidence outside Git and publish only sanitised summaries.

## Run the scorecard

`python3.12 scripts/evaluate_release_measures.py report.json` checks the counts, sample sizes and thresholds. Each report has `revision`, `platform` (`macos`, `windows` or `linux`), `hardware`, `providers`, `speech` (`human` or `generated`) and `measures`. Each measure contains `attempts`, `successes` and, where relevant, `latencies_ms`. `false_wake` instead contains `activations` and `hours`.

The measure keys are `wake`, `false_wake`, `wake_feedback`, `first_reply`, `cancellation`, `task_completion` and `conversation_safety`. Timing measures require one latency for every successful attempt. `task_completion` also requires `unauthorised_actions: 0`. Wake feedback and first reply require at least 20 attempts and 95% success. Cancellation latency is measured only after playback has actually started.

A failing or incomplete report blocks acceptance for that tested configuration until fixed or explicitly accepted with evidence. Generated-only reports cannot establish human voice acceptance. The evaluator returns a nonzero exit status for either case. A release needs reports for all three platforms; run the evaluator on each report. CI runs deterministic lifecycle and evaluator tests; acoustic scorecards are a separate acceptance step, not invented CI results.

Do not carry an old green scorecard forward after changing capture, wake detection, playback, task routing or notification delivery. Keep observed results and accepted exceptions in `docs/validation/`, with the revision and exact tested platform. The previously accepted Linux wake exception remains scoped to the documented distribution and evidence.
