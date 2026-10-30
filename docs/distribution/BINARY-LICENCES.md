# Shipping binary releases

The app source is Apache-2.0. This does not change the licences of linked code, codecs, operating-system libraries or models. Keep `notices/` and `wake/legal/` in every package. The tracked notice manifest is the inventory for Rust and the listed static native/model components.

## Windows

Public Windows releases are built on the official Windows GitHub-hosted runner with its installed Visual Studio/MSVC and Windows SDK. Record the exact runner image, `cl /Bv`, `VCToolsVersion`, SDK version and selected non-debug runtime libraries alongside each release. Retain the applicable Microsoft terms and distributable list from that installation. Microsoft permits distributable `.lib` code to be linked into an application; this does not permit publishing the SDK, headers, toolchain archives or debug runtime as part of notdot.

The private cross-compiled QA binaries have different provenance: cargo-xwin 0.23.1 / xwin 0.10.0, cached Visual Studio 2022 CRT 14.44.17.14 (header version 14.44.35220), and Windows SDK 10.0.26100. These are internal test artifacts, **not public release inputs**. The xwin download/acceptance flag is not evidence of a separate redistribution entitlement. Do not upload that cache or substitute its binaries into the public runner's release output.

Relevant original terms and guidance:

- [Microsoft redistribution guidance](https://www.microsoft.com/licensing/guidance/Visual-Studio)
- [Visual Studio 2022 distributable list](https://learn.microsoft.com/en-us/visualstudio/releases/2022/redistribution)
- [Microsoft licence directory](https://visualstudio.microsoft.com/license-terms/)
- [The licence referenced by xwin 0.10.0](https://go.microsoft.com/fwlink/?LinkId=2086102)

No Microsoft runtime is relicensed under Apache-2.0. A new runner/toolset requires a fresh provenance record; the internal QA version is not a pin for public builds.

## Linux Debian packages

The `.deb` depends on distro-provided GTK/WebKit, GStreamer/codecs, audio and speech packages. It does not redistribute those platform libraries. Their package managers provide their own notices and sources. Keep notdot's bundled Rust/native/model notices; do not copy the AppImage platform-library inventory onto this different distribution boundary.

## Linux AppImage

The working media framework is intentionally retained. Original full common licence texts are tracked in `third_party/linux-common-licenses/`, hash-indexed beside that directory and copied into each AppImage under `usr/share/common-licenses`. The collector verifies their exact presence. This AppImage-only mapping does not overwrite the system-owned common files in Debian packages. Its codec pool includes GPL components, so shipping only copyright files or links to upstream home pages is insufficient. Each exact AppImage must have a companion corresponding-source archive at the **same release download location**, containing:

- the exact notdot revision, lockfile, resources and build scripts, plus vendored locked Rust sources and the pinned native source archives;
- exact distro source packages, including upstream archives, Debian/Ubuntu patches and build recipes, for every redistributed platform ELF library;
- package/version-to-file mappings, checksums, original copyright notices and full referenced common licence texts;
- the retained separate source/notice provenance for notdot's static native components and Rust packages.

Use `scripts/collect_linux_distribution.py` on the **same Debian/Ubuntu builder and AppDir** used for the image. Enable the matching official `deb-src` repositories first. The script fails on unmatched ELF inputs, missing notices, unavailable exact sources, escaping/dangling symlinks, empty/invalid source archives or changed tracked application sources. A known Tauri WebKit `/usr` path relocation is matched narrowly and its modification recipe is retained alongside the original source package. It does not infer licences from filenames. RPATH-patched libraries must retain matching build IDs and matching allocated sections other than the dynamic string/table sections (dynamic symbols are compared semantically); the manifest distinguishes these from byte-identical copies. The excluded tables remain in the actual image and are not presented as byte-identical source provenance.

```sh
python3 scripts/prepare_appimage_source_inputs.py \
  --appdir path/to/AppDir --appimage path/to/notdot.AppImage --squashfs-offset OFFSET \
  --project path/to/clean/exact-notdot-checkout --output path/to/launcher-sources
python3 scripts/collect_linux_distribution.py path/to/AppDir path/to/notdot-corresponding-source \
  --appimage path/to/notdot.AppImage --squashfs-offset OFFSET \
  --app-binary-sha256 BUILD_RECORD_BINARY_SHA256 \
  --project path/to/clean/exact-notdot-checkout \
  --separately-covered path/to/launcher-sources/independently-covered.json
```

The producer checks official AppRun/runtime release assets against the packaged bytes, records immutable source revisions and retains the pinned runtime dependency sources. An unrecognised launcher refuses collection. Runtime metadata slots used by appimagetool are compared separately from executable bytes. The final source manifest binds the exact complete AppImage hash/size, its runtime prefix, executable and source revision.

The separately-covered JSON maps exact hashes only for notdot, its separately inventoried helper/native payload, and independently licensed AppImage launcher inputs to objects containing `notice` and `source` file paths. Both actual files are copied into the source companion; a link or description alone is rejected. An unknown platform library must not be added there to silence a failure. Preserve the JSON with the release record. Publish the resulting `.tar.gz`, checksum and manifest with the AppImage; do not publish a partial output directory from a failed collection. Enable source repositories/install build tools with administrator access first; run the collector as the ordinary build user. The collector needs `dpkg-dev`, `apt`, `git`, Python 3, Cargo and the same installed packages used by linuxdeploy. A post-build source archive is supplemental to the existing notices, not a replacement.

### Replacing shared libraries

An AppImage can be extracted with `./notdot.AppImage --appimage-extract`. The extracted `squashfs-root/AppRun` launches the app. Users may replace bundled shared libraries with interface-compatible modified versions and run the extracted app. notdot does not prohibit modification or reverse engineering for debugging those modifications. The Apache-2.0 application source/build scripts and corresponding platform sources are supplied with the release. Verify this extracted-directory path on the selected final AppImage; a working `.deb` does not establish it.

See [LGPL-2.1 sections 4 and 6](https://www.gnu.org/licenses/old-licenses/lgpl-2.1.html) and [GPL-3.0 section 6](https://www.gnu.org/licenses/gpl-3.0.html). Preserve upstream exceptions and mixed-licence notices. Providing sources does not grant codec patents or replace upstream licence conditions. No speculative three-year written offer is generated: actual downloadable corresponding source is the selected delivery mechanism.

The historical QA AppImage is not a public release candidate. It lacks this complete source companion and referenced common licence texts. Rebuilding a selected public AppImage and successfully collecting/validating its exact inputs is required; the private test image must not be uploaded as the final release.
