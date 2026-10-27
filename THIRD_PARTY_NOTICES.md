# Third-party components

notdot is a development application using open-source Rust dependencies listed with exact versions in Cargo.lock. Mnemara 0.6.0 is used under the MIT license; its notice is retained in third_party/Mnemara-LICENSE.md. Tauri is dual licensed Apache-2.0/MIT. Rust/WASM browser bindings use wasm-bindgen and web-sys. The orb is a Rust reimplementation of the Lumina visual direction; no Electron or Lumina dashboard is shipped.

A public binary release requires a complete dependency-license inventory and retained notices for the selected distribution. No public binary release is being made here.

Desktop lifecycle additions use the Tauri updater/autostart plugins, sysinfo and macOS Security Framework bindings. Their resolved versions are in Cargo.lock. The app logomark is original vector artwork in assets/logomark.svg. Linux AppImage bundles also contain GTK/GStreamer and their runtime dependencies; a complete distribution notice/license inventory is still a public-release gate.

## Retained source and model notices

- First-party source and original logomark: Apache-2.0; see root LICENSE and NOTICE.
- Vendored GLib: its upstream licence is retained in [vendor/glib/LICENSE](vendor/glib/LICENSE); local changes and provenance are recorded alongside it.
- Mnemara: [MIT notice](third_party/Mnemara-LICENSE.md).
- Wake models, Whisper/whisper.cpp/whisper-rs, sherpa-onnx, ONNX Runtime and dictionary: [resource inventory](src-tauri/resources/wake/NOTICE.md), with original licence texts and ONNX third-party notices in that directory.

The root licence does not replace these licences. Cargo.lock is the resolved dependency inventory, not a complete binary attribution bundle. Before distributing binaries, collect the licences of all linked Rust/native dependencies and bundled platform libraries for each target. Provider services and the separately installed Codex runtime are not relicensed or redistributed by this project.
