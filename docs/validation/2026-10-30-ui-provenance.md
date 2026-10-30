# Production UI provenance: 30 October 2026

**Passed:** a fresh production build from the committed orb source matches the current prebuilt frontend. The same WASM and JavaScript are present inside the checked Mac, Linux and Windows application binaries. Stale prebuilt UI is not supported as the explanation for the current Linux wake failure.

## Build evidence

Source and relevant build inputs were unchanged against `66587d1`: orb crate, workspace manifest, Cargo.lock, HTML, CSS, logomark and Cargo configuration. Their combined SHA-256 fingerprint is `7f545dde500f6f43dd503f812173f19cfd18f701ceb203e33e36bbfdd1f21c98`.

A fresh, isolated Cargo target directory rebuilt the WASM dependency graph and orb crate in 22.57 seconds. Production Trunk then generated assets into an ignored alternate directory. No `ui-dist` or packaged application was overwritten.

- Rust 1.97.0; production/default features, without `debug-ui`.
- Two Cargo build jobs; offline Trunk build.
- Existing wasm-bindgen 0.2.129 and wasm-opt 131 binaries were used from the local tool cache.
- The first offline Trunk invocation compiled successfully but could not discover the cached wasm-bindgen executable. Explicitly adding the existing cache directories to PATH completed the asset pipeline; no dependency or source changed.

All six generated files, including the HTML, JS snippet, stylesheet and SVG, match `ui-dist` byte for byte. The production WASM is 487,691 bytes:

`bacbd4b3ab7e0105c351c29a963cec23800a93039150d12860139d56d4a84781`

## Actual binary checks

The check decompressed Tauri's codegen asset files and matched the raw WASM and JS to the fresh build. It then found the exact encoded asset bytes inside each application binary. It also found the encoded HTML entry point referencing the matching fresh JS and WASM filenames. This goes beyond comparing loose prebuilt files.

| Checked binary | SHA-256 | Fresh WASM byte offset |
| --- | --- | ---: |
| Mac production `dede90f` | `7016d7d9e919984d9dfafd66348ddd111656b8f3ba57cd473dfec51380fa82a1` | 27673412 |
| Linux ARM production `dede90f` | `1df2795568c9c8384aa6dfbb593b1b06f3de47fc641647147c23df01ba6b123e` | 34409736 |
| Windows headroom candidate mapped to `66587d1` | `629e36dca9a192d0c6234cd518d648f1ed51caee9ebc956c68c772ee87a28b16` | 30022270 |

The current Windows candidate's retained source-input manifest independently records the same prebuilt frontend inputs. A subsequently rebuilt or instrumented Linux executable needs its own binary identity; these checks do not substitute for identifying the executable actually running during a live test.

## Wake lease boundary

Source inspection found no build delta to explain a different wake contract. The frontend's snapshot/heartbeat loop uses an independent 100 ms timer, with effective polling intervals of 200 ms during wake listening/capture and 500 ms otherwise. It is not driven by the orb animation frame loop. The native lease expires after three seconds.

A heartbeat requires an idle, visible, unmuted, onboarded app with saved voice/STT access and a selected voice; pending speech, open Settings/About or an update installation can pause it. These are intentional gates. Source inspection alone does not prove their live values, timer cadence, continuous lease renewal or native detector timing. Those remain part of root's Linux runtime diagnosis.

## Evidence and limits

Ignored evidence is under `.local/ui-provenance/`: build logs, `source-inputs.json`, `comparison.json` and `entrypoint-comparison.json`. The check used only source, compiler tools and local binary files. It did not launch the app, access credentials/providers, open UI, or use audio. It does not establish that the full Linux wake turn passes.
