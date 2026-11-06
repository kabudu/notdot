# Speaker recognition model

`3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx`

- Model: CAM++ speaker verification, "common advanced" (Chinese and English, 16 kHz, 192-dimensional embeddings), from the 3D-Speaker project by Alibaba DAMO Academy / Tongyi Lab (https://github.com/modelscope/3D-Speaker; ModelScope `iic/speech_campplus_sv_zh_en_16k-common_advanced`).
- Licence: Apache License 2.0 (see LICENSE). The ModelScope model card lists Apache License 2.0.
- ONNX export: k2-fsa/sherpa-onnx release `speaker-recongition-models` (Apache-2.0).
- SHA-256: see SHA256SUMS. notdot verifies size and checksum before loading.
- Runs entirely on this device. notdot keeps only encrypted embeddings, never enrolment audio.
