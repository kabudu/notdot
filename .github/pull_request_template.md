<!-- Explain the final change for someone who has not followed the discussion.
Keep small PRs short. Remove sections that do not apply; explain any untested path.
Never include API keys, private conversations or recordings without permission. -->

## What changes for the user?

<!-- Describe the problem or request, then the resulting behaviour.
A concrete before/after example is useful. Link the issue or roadmap item. -->

Related issue or roadmap item:

## What changed?

<!-- Outline the important changes and any tradeoff a reviewer should assess.
Explain new dependencies or a change to a shared interface. -->

## How was it checked?

<!-- Include the commands or steps, result, build/revision and relevant platforms.
For voice work, distinguish a real microphone test, generated audio and mocks.
Record observed response/cancellation timing when performance is affected.
For documentation or website changes, check links and relevant layouts. -->

| Check | Platform / build | Result |
| --- | --- | --- |
| | | |

Untested paths and why:

## Interface and accessibility

<!-- If relevant: show the changed state with a screenshot or short staged clip.
Check keyboard use, focus, reduced motion and readable labels. Explain how voice
feedback and error recovery work. Keep the shipped conversation interface voice-only. -->

## Privacy, permissions and stored data

<!-- If relevant: describe changes to microphone/background listening, network
requests, keys, memories, connected tools or other account access. Explain consent,
cancellation, migrations and what happens to existing settings/data. Never paste secrets. -->

## Compatibility and release impact

<!-- If relevant: cover affected operating systems, provider/runtime API changes,
installer/update/restart behaviour, native CPU requirements and new dependencies.
Retain original licences/notices and account for redistributed binaries or media. -->

## Contributor checks

- [ ] The PR explains the user-visible outcome and is focused enough to review.
- [ ] Relevant checks passed; the evidence above identifies gaps honestly.
- [ ] User documentation, roadmap and changelog are updated where needed.
- [ ] No secrets, private data or media without publishing rights were added.
- [ ] Relevant permission, compatibility and third-party licence changes are explained.
