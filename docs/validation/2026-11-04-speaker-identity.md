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

## Replay (honest limits)

| Attack | Result |
|---|---|
| Verbatim replay of an enrolment clip | 32 of 32 flagged as a suspected replay and refused |
| Enrolment clip through a simulated speaker and room (low-pass, echo, noise) | 16 accepted, 16 ambiguous: **not caught** |
| A different recorded phrase through the same simulated channel | 5 accepted, 3 ambiguous: **not caught** |

notdot has no anti-spoofing. Anyone with a recording of an enrolled person, or a clone of their voice, may be accepted. The product copy and settings must say so. Device controls (people, voices, modes, sharing, keys, reset) never accept a voice; they need a deliberate action in notdot's window.

## Privacy and policy tests

`profiles.rs` (14 tests), `notdot-speakers` (16 unit tests) and the native adapter (6 tests, including a mid-inference access-change race, plus one end-to-end test that uses the model). All pass in the release profile, with Clippy `-D warnings` clean.

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
  - their voiceprint,
  - their shares,
  - their roster entry,
  - their receipts and queued announcements,
  - `profiles/<id>/`.

  The owner's ledger is untouched. Their companion chat ID is returned so it can be archived in Codex.

## Coverage gaps (not yet evidence)

- Real human voices, accents, children, illness, whispering, far-field microphones and real rooms.
- Real loudspeaker replay and voice clones. Expect acceptance.
- Windows and Linux runtime timing (the same sherpa-onnx static build ships there, but this was not measured).
- Long-term voice drift and re-enrolment cadence.
- The UI enrolment flow on a device with consenting people. This needs an audio slot and participants.
