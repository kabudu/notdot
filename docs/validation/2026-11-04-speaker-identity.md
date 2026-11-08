# Speaker identity and private profiles: evidence, 4 November 2026

Scope: on-device speaker recognition (`crates/notdot-speakers`), profile and access policy (`crates/notdot-core/src/profiles.rs`) and the native adapter (`src-tauri/src/speakers*`). Host: Apple Silicon Mac, macOS 27, Rust 1.97, release profile.

## Summary

- Speaker embeddings come from the 3D-Speaker CAM++ "common advanced" model (Apache-2.0), run through sherpa-onnx 2.0.0. They are computed on the device and checked against a pinned SHA-256 before loading.
- Voiceprints are 192-dimensional embeddings, encrypted at rest with ChaCha20-Poly1305. The key is kept in the OS keychain. notdot never stores enrolment or identification audio.
- **All acoustic figures below come from synthetic voices** (macOS system voices rendered to files). No real person was recorded or enrolled, and nothing was played aloud. Real-voice, real-room and real-replay performance is **not yet measured**.
- Voice recognition is a convenience identity, not authentication. A recording or a voice clone played near the microphone can be accepted (see Replay).

## Model selection

Three Apache-2.0 3D-Speaker exports from the sherpa-onnx `speaker-recongition-models` release were scored on the same synthetic set. Each speaker's centroid was built from 4 enrolment phrases, then scored against 8 other phrases.

| Model | Lowest genuine score | Median genuine | Median impostor | Approx. EER | Time per clip (1 thread) |
|---|---|---|---|---|---|
| CAM++ en VoxCeleb (`…campplus_sv_en_voxceleb_16k`) | 0.435 | 0.859 | 0.694 | 21.9 % | 47 ms |
| ERes2Net en VoxCeleb | 0.764 | 0.874 | 0.176 | 0.8 % | 122 ms |
| **CAM++ zh/en common advanced** (chosen) | **0.819** | **0.892** | **0.259** | **0.8 %** | **43 ms** |

The English VoxCeleb CAM++ export, which I planned to use at first, does not separate speakers in this pipeline. It also failed on sherpa-onnx's own reference clips: two different people scored 0.88 against each other and one person scored 0.58 against themself. Doubling the input scale changed nothing, because the model normalises samples. I did not investigate it further. The chosen model:
- file `3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx`
- size 28,281,164 bytes
- SHA-256 `aa3cfc16963a10586a9393f5035d6d6b57e98d358b347f80c2a30bf4f00ceba2`

Embedding the original audio, from the first to the last voiced frame, measured better than splicing voiced frames together, so the shipped code does that.

## Synthetic evidence set

`crates/notdot-speakers/scripts/render-synthetic-voices.sh` renders 16 macOS voices to 16 kHz WAV. Each voice gets 4 enrolment phrases (the guided-enrolment prompts) and 8 different test phrases. The voices cover these accents: en-GB, en-AU, en-IE, en-IN, en-US and en-ZA.

- **Enrolled** (through the real guided-enrolment code): Daniel, Karen, Moira, Rishi, Samantha, Tessa, Eddy, Flo.
- **Never enrolled**: Aman, Fred, Ralph, Kathy, Reed, Grandpa, Grandma, Albert.

Some voices share a synthesiser engine and differ only in parameters: Eddy, Flo, Reed, Grandpa and Grandma; Aman and Rishi; probably Samantha and Karen. They stand in for very similar real voices, such as close relatives, and their results are reported separately.

Reproduce:

```text
crates/notdot-speakers/scripts/render-synthetic-voices.sh /tmp/voices
NOTDOT_SPEAKER_MODEL=…/3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx \
NOTDOT_SPEAKER_FIXTURES=/tmp/voices \
cargo test -p notdot-speakers --release --test acoustic -- --ignored --nocapture
```

## Calibration

The default thresholds are accept 0.70, reject 0.50, margin 0.10 and replay 0.985. A score between reject and accept, or one within the margin of the runner-up, is reported as ambiguous and refused in locked modes.

- Genuine centroid scores (n = 64): min 0.769, median 0.883.
- Impostor scores (n = 960): median 0.247, max 0.886. The maximum is a same-engine pair.
- Score-level error rates at 0.70: FRR 0 %, FAR 1.1 %. Every impostor score above 0.70 is a same-engine pair.

At 0.60, guided enrolment refused Flo and Samantha as "too similar" to Eddy and Karen. At 0.70 it accepted both. That refusal is deliberate behaviour for people who cannot be told apart.

## Decision results (clean audio unless stated)

| Condition | Enrolled voices (n = 64) | Unknown voices (n = 64) |
|---|---|---|
| Clean | 64 correct, 0 confused | 0 unrelated accepted; 8 same-engine accepted (Reed as Eddy); 24 ambiguous; 32 unknown |
| White noise, 20 dB SNR | 52 correct, 12 ambiguous | 2 same-engine accepted (Reed as Eddy) |
| White noise, 10 dB SNR | 18 correct, 42 ambiguous, 4 unknown | 0 accepted |
| White noise, 0 dB SNR | 0 correct, 18 ambiguous, 46 unknown | 0 accepted |
| Competing talker at −6 dB | 51 correct, 9 ambiguous, 4 unknown | 0 accepted |

No enrolled voice was ever identified as a different enrolled person. In noise the system refuses ("I couldn't quite tell who that was") rather than guessing. In owner-only and trusted modes that means false rejections in loud rooms. Microphone mute, keyboard access and switching to open mode on the device are the recovery paths.

Embedding time: median 37 ms, p95 50 ms, maximum 85 ms per test clip on one thread. Model load and checksum: about 180 ms.

## Short requests (2026-11-07)

A real owner test at desk distance showed that short commands ("what is seven plus five") reach only about 0.6 s of voiced frames. Every one was refused as unclear under the 1.2 s minimum. Requests now need 0.5 s of voiced speech. Enrolment samples and continuity windows keep 1.2 s. `tests/short_requests.rs` cuts each test clip to N seconds from speech onset, at the measured voiced level (RMS 0.019):

| Clip | Enrolled (n = 64) | Unknown (n = 64) |
|---|---|---|
| 0.6 s | 6 correct, 54 ambiguous, 4 unknown | 1 same-engine accepted |
| 0.8 s | 38 correct, 25 ambiguous, 1 unknown | 6 same-engine accepted |
| 1.0 s | 57 correct, 7 ambiguous | 6 same-engine accepted |
| 1.3 s | 61 correct, 3 ambiguous | 8 same-engine accepted |
| 2.5 s | 64 correct | 8 same-engine accepted |

Shorter audio makes matching less certain, so more requests are refused as ambiguous. It does not confuse enrolled people or accept new voices: every accept of an unknown voice is Reed as Eddy, the same pair as at full length. After the change, the full acoustic evaluation gives exactly the figures above.

## Replay (honest limits)

| Attack | Result |
|---|---|
| Verbatim replay of an enrolment clip | 32 of 32 flagged as a suspected replay and refused |
| Enrolment clip through a simulated speaker and room (low-pass, echo, noise) | 16 accepted, 16 ambiguous: **not caught** |
| A different recorded phrase through the same simulated channel | 5 accepted, 3 ambiguous: **not caught** |

notdot has no anti-spoofing. Anyone with a recording of an enrolled person, or a clone of their voice, may be accepted. The product copy and settings must say so. Device controls (people, voices, modes, sharing, keys, reset) never accept a voice; they need a deliberate action in notdot's window.

## Privacy and policy tests

`profiles.rs` (18 tests), `notdot-speakers` (16 unit tests) and the native adapter (6 tests, including a mid-inference access-change race, plus one end-to-end test that uses the model). All pass in the release profile, with Clippy `-D warnings` clean.

- Profile IDs are random UUIDv4 values and cannot be names or path fragments. Renaming a person never moves their data.
- Namespaces:
  - The owner keeps `<app_data>/memory`.
  - Members use `<app_data>/profiles/<id>/{memory,state}`.
  - Companion threads can't be reused across people or shared.
- Chat access control:
  - Members see and read only granted chats.
  - The first grant is read-only. Control is a separate second step, still capped by the global ReadOnly setting.
  - Members' private chats are hidden from the owner as well.
  - Discovery is filtered.
- Voices never gain authority: owner and member voices are refused all of these: managing members, changing the mode, sharing chats, changing credentials, reset.
  - An owner voice may read and change ordinary device settings (with the existing confirmation); a member voice may not.
- Revocation mid-turn: each of the following invalidates the affected in-flight locks, receipts and queued announcements:
  - removing a member
  - switching to owner-only
  - downgrading or revoking a share
  - re-enrolling
  - open → locked, for open-fallback locks
- Outbox: completion reports and reminders are released only to the profile that asked for them, and dropped if access changed. Each profile is capped at 32 items.
- Locked modes fail closed when:
  - the model is missing,
  - the voice key is missing or wrong,
  - the voiceprint file is tampered with,
  - the model has changed,
  - inference fails or times out (2 s),
  - `profiles.json` is damaged.

  Typed requests still work, except with a damaged roster, which needs one local recovery step.
- Encrypted at rest:
  - The voiceprint file contains no profile ID, name or float pattern in plaintext.
  - Unix permissions are 0600.
  - Every write uses a fresh nonce.
  - A one-bit change anywhere is detected.
- Restart: voices persist through the keychain. A lost key leads to "Forget all voices", which keeps people, memories and shares.
- Cancelled enrolment leaves no file and no samples. Enrolment and identification audio are zeroised after embedding.
- Removing a person deletes exactly:
  - their voiceprint (works without the speaker model; if the keychain is unavailable, the voice is never used and is deleted the next time voices load),
  - their shares,
  - their roster entry,
  - their receipts, pending self-removal and queued announcements,
  - `profiles/<id>/`.

  The owner's ledger is untouched. Messages they added to shared owner chats are left in those chats.

## Offboarding (6 November)

Authority is checked natively in `Speakers::authorise_removal`, which is the only way to obtain the ticket that `remove_member` consumes:
- The actor must be a click in notdot's visible, focused window. A voice never qualifies, including the owner's, and the UI never states who is acting.
- The target must be a current trusted member. The principal owner can't be removed by either route, and a recorded removal naming the owner is refused at startup.
- The typed confirmation must match the target's current name.
- Self-service additionally needs the member's own spoken request, a fresh four-digit code spoken back in a *different* capture admitted as that member within two minutes, and the on-screen step within two minutes after that. Everything is bound to the member's current epoch and is single-use. Three wrong codes, expiry, re-enrolment or cancellation end the request. Ordinary speech during a pending request is not treated as a wrong code.

Replay limit: a pre-made recording cannot contain a code that didn't exist when it was recorded, and the last step needs someone at the computer. A real-time voice clone of the member, by someone at the computer, would still pass. Removal is irreversible, so this is a deliberate trade-off between self-service and safety; the owner can always remove members directly.

Retired chats: a removed member's private chat ID is kept in `profiles.json` (ID only, no name) and denied to everyone for every operation, including the owner; it can't be shared or reused as anyone's companion chat, and a hand-edited roster that tries is treated as damaged. Archiving in Codex (`thread/archive`) is attempted at once and retried with backoff from 30 seconds up to hourly until it succeeds. A thread Codex no longer has counts as archived. The panel shows how many are still being archived.

Crash safety: the durable removal marker is written and synced *before* the roster changes. At startup, every marked removal is completed in the roster first; the private folder is deleted only once the roster no longer has the person, otherwise the marker is kept for the next attempt. Tests cover a crash after the marker, after the roster change and after folder deletion, each followed by a restart, plus the case where the roster still has the person. Re-adding someone with the same name gets a new ID, no voice, no consent record, no shares and no companion chat.

## Speaker changes within one capture (6 November)

Admission still identifies the speaker from the first 2–6 seconds, before any audio leaves the computer in locked modes. The whole capture (up to 60 s) is then checked locally in half-overlapping 3-second windows before the transcript can reach anyone's private context. A window fails when another enrolled person clears the accept threshold and beats the admitted person by the margin, or when the admitted person's own score falls below 0.45. Captures shorter than one assessable window rely on admission alone. A switch, an unavailable model or a timeout rejects the capture ("I heard more than one voice, so I didn't send that.") and the request is not sent. Receipts for recognised people are unusable until this check passes; open-mode fallbacks keep legacy behaviour.

Measured on spliced and mixed synthetic voices (8 enrolled, 4 unenrolled; 32 captures per row; `crates/notdot-speakers/tests/continuity.rs`):

| Scenario | Stopped at admission | Stopped by the whole-capture check | Mixed captures sent |
|---|---|---|---|
| One enrolled person, two phrases | 0 | 0 | — |
| One enrolled person, 20 dB noise | 0 | 0 | — |
| One enrolled person, 10 dB noise | 16 | 0 | — |
| A for 2.5 s, then enrolled B | 0 | 32 | 0 |
| A for one phrase, then enrolled B | 0 | 31 | 1 |
| A for 2.5 s, then an unenrolled guest | 0 | 31 | 1 |
| A and B overlapping at equal level | 3 | 2 | 27 |
| A with B 6 dB quieter underneath | 0 | 4 | 28 |
| A with a guest 6 dB quieter underneath | 1 | 5 | 26 |

Handovers are caught; simultaneous speech mostly is not, because a mixture usually still resembles one speaker. I tried an extra rule that flags any window where another enrolled voice scores 0.65 or more. It caught 8 of 32 equal-level overlaps but also stopped 4 of 32 single-speaker captures, so it was not adopted. Overlap is a documented limit: the guidance tells people to speak one at a time.

## Name use

The model is given the speaker's name only when they were actually recognised and have a real name. An unrecognised voice served in Anyone mode is not called by the owner's name, and the owner's placeholder name "Me" is never used. The instruction asks for occasional, natural use rather than starting every reply with it. Teaching the owner's voice requires a real name first.

## Coverage gaps (not yet evidence)

- Real human voices, accents, children, illness, whispering, far-field microphones and real rooms.
- Real loudspeaker replay and voice clones. Expect acceptance.
- Windows and Linux runtime timing (the same sherpa-onnx static build ships there, but this was not measured).
- Long-term voice drift and re-enrolment cadence.
- The UI enrolment flow on a device with consenting people. This needs an audio slot and participants.
- Real interruptions and overlapping conversation between real people, and self-service removal spoken by a real person.
