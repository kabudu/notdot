# Licensed synthetic wake fixtures

These five fixed phrases were rendered offline with the neural Piper `en_US-ljspeech-medium` model. They contain no user recording or Apple System Voices audio. All files are mono signed 16-bit PCM WAV at **16 kHz**, produced by actual resampling from 22,050 Hz.

| File | Spoken phrase | Expected with “Hey Ava” | Length scale |
| --- | --- | --- | ---: |
| hey-ava.wav | Hey Ava. | Accept | 1.15 |
| hey-ava-uk.wav | Hey Ava | Accept | 1.0 |
| hey-ava-us.wav | Hey Ava | Accept | 0.85 |
| ava.wav | Ava | Reject: missing “Hey” | 1.0 |
| negative.wav | The weather is pleasant today | Reject | 1.0 |

The UK/US filename suffixes are retained as compatibility IDs. These are **one US-English model with timing variations**, not a new three-accent test or a claim about real users.

Piper’s default dictionary pronounced this name `/ˈɑːvə/`. For these fixed fixtures, explicit IPA input supplies the intended `/ˈeɪvə/`; the complete input phonemes and derived configuration are retained. Model weights, production detector, recognition thresholds and negative expectations were unchanged. No output gain was applied.

## Source and terms

- **Model:** the [pinned Piper voices repository](https://huggingface.co/rhasspy/piper-voices/tree/5041a7a49ae5b3a0f30e6837769eb03fdbb231bd) declares MIT. Its original [declaration](voice-synthesis/PIPER-VOICES-README.md) and [LJSpeech model card](voice-synthesis/LJSPEECH-MODEL-CARD.md) are retained.
- **Synthesis engine:** Piper release `2023.11.14-2`, [source revision](https://github.com/rhasspy/piper/tree/38917ffd8c0e219c6581d73e07b30ef1d572fce1), MIT; the original [licence](voice-synthesis/PIPER-ENGINE-LICENSE.md) is retained. Its binary bundle has separately licensed dependencies; synthesis tools are not distributed here.
- **Training data:** [LJ Speech](https://keithito.com/LJ-Speech-Dataset/), credited to Keith Ito and Linda Johnson / LibriVox. The publisher and model card identify it as public domain; the publisher specifies US public-domain status.
- **Generated fixtures:** notdot project test assets under Apache-2.0, with the upstream source declarations retained above.

[VOICE-FIXTURES.json](VOICE-FIXTURES.json) records immutable source URLs/revisions, model/tool/config hashes, pronunciation settings, conversion parameters and every final WAV hash. The model hash was checked against its published LFS object ID. Model weights and synthesis binaries are not added to the repository or shipped app.

The native persistent-helper corpus test checks all five clips in one model process. This fixed synthetic test does not prove hardware, accent, arbitrary-name, human-hearing or real-world false-activation performance. Earlier results that used private-development clips retain their original scope and must not be relabelled as results for these bytes.
