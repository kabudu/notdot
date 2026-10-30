# Third-party licences and notices

notdot’s own source and original artwork are licensed under **Apache-2.0**; see [LICENSE](LICENSE) and [NOTICE](NOTICE). Third-party components keep their own licences.

The app packages a [licence index](src-tauri/resources/notices/INDEX.md), original copyright notices and full standard terms under `notices/` in its resources. The [manifest](src-tauri/resources/notices/manifest.json) records the locked Rust packages, source archives, published revisions and text checksums. It retains every declared licence choice, including upstream declarations; standard template placeholders do not invent copyright holders.

This covers the conservative union of the native-app and WASM Rust dependency graphs, the optional Windows helper’s shared runtime, the Rust standard-library notice, and the listed known static native/model components. Mnemara’s original [MIT notice](third_party/Mnemara-LICENSE.md), vendored [GLib licence](vendor/glib/LICENSE) and [wake/model notices](src-tauri/resources/wake/NOTICE.md) remain intact. The orb and logomark are original notdot implementations; no Lumina dashboard is shipped.

Maintainers can regenerate the index offline with `python3 scripts/distribution_notices.py --write` and validate it with `python3 scripts/distribution_notices.py`. Local CI checks the retained hashes, declared licence mappings and lockfile identity. A changed dependency lock needs an updated inventory and verified source texts; changing only the recorded lock hash is insufficient.

Windows packages also carry the original installer notices under `windows-notices/`. AppImages include original platform and runtime credits and full referenced licence texts, with a matching corresponding-source archive alongside the download. The release workflow records Windows toolset provenance and checks the Linux source/binary pairing; see [binary distribution details](docs/distribution/BINARY-LICENCES.md).

The [test audio provenance](src-tauri/tests/fixtures/VOICE-FIXTURES.md) records its openly licensed synthesis sources. Provider services and the separately installed Codex runtime are not redistributed or relicensed by notdot.
