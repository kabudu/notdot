# ONNX Runtime C API headers (vendored, pinned)

The official headers from ONNX Runtime v1.28.2, unmodified, from
`https://raw.githubusercontent.com/microsoft/onnxruntime/v1.28.2/include/onnxruntime/core/session/<file>`:

| File | SHA-256 |
| --- | --- |
| `onnxruntime_c_api.h` | `b69a133143c0da61782b2b02cc8a620d21ac139231fd211719776d10385135c1` |
| `onnxruntime_ep_c_api.h` (included by the above) | `db86df0f846b8d3bdbf429a5c76c5805293196575ca3c475878612591cd18e51` |
| `onnxruntime_error_code.h` (included by the above) | `5ce3b054e798eced8d14f5b86e98692fd33470463f96194ce0700a2d53dd8721` |

`build.rs` checks all three digests before compiling; the shim does not build
against anything else. Licence: MIT, Copyright (c) Microsoft Corporation
(`LICENSE`, from the same tag, SHA-256
`2f07c72751aed99790b8a4869cf2311df85a860b22ded05fa22803587a48922c`).

Only `../ort_shim.c` includes them, and only with the `personal-phonetic` feature. No
runtime is vendored: the shim calls the exported `OrtGetApiBase` of the ONNX
Runtime that sherpa-onnx 2.0.0 already links (1.28.2, statically on macOS
hosts and as `libonnxruntime.so` on Android), requesting C API version 16, so
every function the shim uses exists in that runtime. iOS (the runtime inside
the sherpa-onnx xcframework) has not been built or tested with this feature.
