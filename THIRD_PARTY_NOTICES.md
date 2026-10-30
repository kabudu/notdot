# Third-party licences and notices

notdot’s own source and original artwork are licensed under **Apache-2.0**; see [LICENSE](LICENSE) and [NOTICE](NOTICE). Third-party components keep their own licences.

The app now packages a [licence index](src-tauri/resources/notices/INDEX.md), original copyright notices and full standard terms under `notices/` in its resources. The [manifest](src-tauri/resources/notices/manifest.json) records the locked Rust packages, source archives, published revisions and text checksums. It retains every declared licence choice, including upstream declarations; standard template placeholders do not invent copyright holders.

This covers the conservative union of the native-app and WASM Rust dependency graphs, the optional Windows helper’s shared runtime, the Rust standard-library notice, and the listed known static native/model components. Mnemara’s original [MIT notice](third_party/Mnemara-LICENSE.md), vendored [GLib licence](vendor/glib/LICENSE) and [wake/model notices](src-tauri/resources/wake/NOTICE.md) remain intact. The orb and logomark are original notdot implementations; no Lumina dashboard is shipped.

Maintainers can regenerate the index offline with `python3 scripts/distribution_notices.py --write` and validate it with `python3 scripts/distribution_notices.py`. Local CI checks the retained hashes, declared licence mappings and lockfile identity. A changed dependency lock needs an updated inventory and verified source texts; changing only the recorded lock hash is insufficient.

Separate public-distribution checks remain for Windows static CRT/SDK terms and Linux AppImage platform-library provenance, full terms and source availability. The package-managed Linux dependency boundary differs from AppImage redistribution. This bundle does not claim to settle those checks or establish legal clearance. Provider services and the separately installed Codex runtime are not redistributed or relicensed by notdot.
