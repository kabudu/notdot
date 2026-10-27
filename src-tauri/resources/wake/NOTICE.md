# Bundled local wake detection

The selected ONNX weights and tokenisation resources come from k2-fsa/sherpa-onnx's official `kws-models` release, under Apache-2.0 (see LICENSE). These are inference resources, not a hosted service. Only one selected keyword model runs at a time. A speech-gated local recognizer supplements it when a spoken phrase is missed.

- English BPE fallback: `sherpa-onnx-kws-zipformer-gigaspeech-3.3M-2024-01-01`, quantised encoder/joiner and floating-point decoder, plus bpe.model and tokens.txt.
  https://github.com/k2-fsa/sherpa-onnx/releases/download/kws-models/sherpa-onnx-kws-zipformer-gigaspeech-3.3M-2024-01-01.tar.bz2
- Dictionary pronunciation model: `sherpa-onnx-kws-zipformer-zh-en-3M-2025-12-20`, chunk-8 quantised encoder/joiner and floating-point decoder, tokens.txt and en.phone. This application currently configures English wake phrases only.
  https://github.com/k2-fsa/sherpa-onnx/releases/download/kws-models/sherpa-onnx-kws-zipformer-zh-en-3M-2025-12-20.tar.bz2

SHA256SUMS in each directory pins every model/tokenisation asset; the native build checks these before packaging. The filename changes are recorded above; weights are unmodified. The pronunciation dictionary uses CMU-style English pronunciations; its attribution/license is included in legal/CMUDICT-LICENSE.

Native inference uses the official Apache-2.0 sherpa-onnx Rust/C API 2.0.0, statically linked by sherpa-onnx-sys. Its build script obtains the platform's static archive from the official versioned release (or SHERPA_ONNX_ARCHIVE_DIR for an offline build). Native archives are build inputs, not runtime downloads. The macOS arm64 archive contains ONNX Runtime 1.28.2; its MIT license and upstream third-party notices are included in legal/. These notices supplement the project's public distribution audit; complete per-platform static dependency notices remain a release gate.

https://github.com/k2-fsa/sherpa-onnx/tree/v2.0.0
https://github.com/microsoft/onnxruntime/tree/v1.28.2
https://github.com/cmusphinx/cmudict

## Local speech fallback

The bundled `ggml-tiny.en-q5_1.bin` (32,166,155 bytes) is the English Whisper tiny model quantised by whisper.cpp. Source: https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en-q5_1.bin . SHA-256: `c77c5766f1cef09b6b7d47f21b546cbddd4157886b3b5d6d4f709e91e66c7c2b`, verified against the distributor's linked-object SHA-256. The build checks the same digest before packaging. Whisper weights are MIT licensed; see WHISPER-LICENSE. Quantised weights are shipped unchanged.

Rust integration uses whisper-rs 0.16.0 (Unlicense) and its whisper.cpp native implementation (MIT); original notices are retained under legal/. Resampling uses rubato 0.16.2 (MIT), resolved in Cargo.lock. No runtime download, external CLI, cloud speech recognition, GPU requirement or API key is needed for local wake recognition. Each speech window is ephemeral and capped at six seconds; one inference job runs at a time on two CPU threads with cancellation and a three-second inference deadline. Native recognizer logging is disabled so background speech is not printed to application logs.
