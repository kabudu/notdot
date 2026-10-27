# Bundled local wake detection

The selected ONNX weights and tokenisation resources come from k2-fsa/sherpa-onnx's official `kws-models` release, under Apache-2.0 (see LICENSE). These are inference resources, not a hosted service. Only one selected model runs at a time.

- English BPE fallback: `sherpa-onnx-kws-zipformer-gigaspeech-3.3M-2024-01-01`, quantised encoder/joiner and floating-point decoder, plus bpe.model and tokens.txt.
  https://github.com/k2-fsa/sherpa-onnx/releases/download/kws-models/sherpa-onnx-kws-zipformer-gigaspeech-3.3M-2024-01-01.tar.bz2
- Dictionary pronunciation model: `sherpa-onnx-kws-zipformer-zh-en-3M-2025-12-20`, chunk-8 quantised encoder/joiner and floating-point decoder, tokens.txt and en.phone. This application currently configures English wake phrases only.
  https://github.com/k2-fsa/sherpa-onnx/releases/download/kws-models/sherpa-onnx-kws-zipformer-zh-en-3M-2025-12-20.tar.bz2

SHA256SUMS in each directory pins every model/tokenisation asset; the native build checks these before packaging. The filename changes are recorded above; weights are unmodified. The pronunciation dictionary uses CMU-style English pronunciations; its attribution/license is included in legal/CMUDICT-LICENSE.

Native inference uses the official Apache-2.0 sherpa-onnx Rust/C API 2.0.0, statically linked by sherpa-onnx-sys. Its build script obtains the platform's static archive from the official versioned release (or SHERPA_ONNX_ARCHIVE_DIR for an offline build). Native archives are build inputs, not runtime downloads. The macOS arm64 archive contains ONNX Runtime 1.28.2; its MIT license and upstream third-party notices are included in legal/. These notices supplement the project's public distribution audit; complete per-platform static dependency notices remain a release gate.

https://github.com/k2-fsa/sherpa-onnx/tree/v2.0.0
https://github.com/microsoft/onnxruntime/tree/v1.28.2
https://github.com/cmusphinx/cmudict
