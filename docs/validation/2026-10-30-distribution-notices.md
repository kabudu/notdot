# Distribution notices: 30 October 2026

This report preserves the initial inventory and its completed follow-ups. The final paired AppImage/source evidence and original full-term packaging are recorded below; the initial gaps describe the state before those changes.

## What was checked

Cargo.lock SHA-256: `40011c8b0852df85ed6b8eaefacaab7effbef89afa27c72dcc18b9ba72e62469`. The lockfile did not change between the application freeze and this inventory. Source revision when the combined audit artifact was assembled: `66587d1`.

| Scope | Packages in the normal dependency graph |
| --- | ---: |
| macOS arm64 app | 350 |
| Windows x64 app | 331 |
| Linux x64 app | 398 |
| WASM orb | 31 |
| Combined, without duplicates | 477 |

These are Cargo metadata graphs, not a claim that every package contributes machine code. Build and test dependencies are excluded; Cargo feature unification can conservatively include extra normal dependencies. Native libraries and platform packages need their own inventory. The optional Windows helper uses the shared wake engine; its packaging and native toolchain also need to appear in the final distribution record.

An ignored evidence artifact is in `.local/notices/artifact/`. It contains:

- A package-to-text index with names, versions, declared licences, target graphs, source repositories and available published VCS revisions.
- Original licence and notice files from the cached packages, supplemented by original upstream files fetched at those exact VCS revisions.
- Source-archive hashes and original licence files for six additional sherpa native dependencies.
- The installed macOS Rust standard-library copyright document, and 203 copyright files extracted from the actual historical Linux AppImage.
- SHA-256 checksums for every artifact file. The JSON index contains no build-machine home paths.

The artifact is explicitly marked incomplete. A file named LICENSE can contain only a declaration or link; text presence alone is not a licence-completeness check. Source files such as `copying.rs` were excluded from the licence-file collector. No copyright holders or licence grants were invented.

## Existing coverage and precise gaps

| File or shipped component | Evidence | Remaining work |
| --- | --- | --- |
| Workspace manifests | The five missing inherited licence fields were fixed in `e6c6db5`; all owned packages declare Apache-2.0. | Include the project LICENSE and NOTICE in the final binary notice bundle. A manifest field alone does not package those files. |
| `third_party/Mnemara-LICENSE.md` | Byte-for-byte match with LICENSE.md at published crate VCS revision `95e3c73bed662d36587f70c15fa3710cfd7516cc`. | Map this notice to mnemara, mnemara-core and mnemara-store-sled in the generated binary index. |
| `src-tauri/resources/wake/legal/` | Existing original ONNX Runtime notices, CMU dictionary, Whisper, whisper.cpp and Silero texts are retained. | Keep these originals; do not replace them with the project licence. |
| sherpa static native libraries | The actual macOS and Windows archives contain Kaldi decoder/fbank, OpenFst, KissFFT, simple-sentencepiece and utf8proc libraries in addition to sherpa and ONNX Runtime. Their original source notices were recovered from hash-verified upstream archives. | Add these recovered notices and provenance to the bundle. The current wake/legal directory does not contain the complete additional set. |
| Rust dependency notices | 287 distinct candidate text files were recovered from the combined cache before upstream supplements. Thirty-two packages had no standalone cached licence file. Exact-revision upstream recovery covers many of these. | Finish the per-package text mapping; retain copyright notices and complete terms rather than treating SPDX names as full texts. |
| objc2 family | Exact published-revision root LICENSE.md files explain the MIT/triple-licence choices and link to the terms. They are declarations, not complete standalone licence texts. | Complete the licence-text assembly and preserve the upstream declaration, including its Apple SDK qualification. Do not insert guessed copyright lines. |
| fxhash 0.2.1 | Published `lib.rs` retains its Rust Project copyright/licence header; the crate and repository omit standalone licence files. | Retain that original header and the complete terms for the selected declared licence option. |
| selectors 0.38.0 | The source has MPL-2.0 headers, but no standalone licence file was found in the package or exact-revision repository root. | Supply the full MPL-2.0 text and record where recipients can obtain the corresponding published source. |
| whisper-rs-sys 0.15.0 | Its packaged whisper.cpp LICENSE covers the native implementation, not the sys crate's declared Unlicense. Both Rust crates share VCS revision `7558e1b72f54f2f22a53589afb77e65681834c36`. | Map the genuine workspace Unlicense from whisper-rs to the sys crate as well; this relationship is recorded in the artifact. |
| Rust standard library | The installed macOS Rust 1.97.0 toolchain supplies a self-contained `COPYRIGHT-library.html`, copied into the artifact. | Include the appropriate standard-library notice for each actual build toolchain. Do not assume that a compiler's entire dependency inventory is shipped by the app. |
| macOS platform frameworks | The checked application dynamically uses Apple frameworks and `/usr/lib`; no separate dylibs are copied into the bundle. | Keep OS-provided libraries distinct from statically linked native components. |
| Windows static CRT / SDK inputs | The release uses static CRT linkage. Absence of non-system DLL imports does not make the static CRT a project-owned component. | Record the actual toolchain/redistributable provenance and applicable terms separately from open-source Rust notices. |

## Actual Linux AppImage findings

The retained x64 AppImage `.local/linux-e2e/notdot_0.1.0_amd64.AppImage` has SHA-256 `5717d1acd51b828671fde91feda504a5c74475ee80d543b2910138cb918b17fb`. Its SquashFS starts at offset 944632. A bounded, network-disabled tools container listed its contents and extracted only `usr/share/doc`; no app, display, microphone or provider was started.

The image contains 443 library-related paths under `usr/lib` and **203 package copyright files**. Those existing notices should be preserved. However, it contains **no `usr/share/common-licenses` entries**.

Concrete examples:

| Bundled files | Retained copyright file | Missing referenced text |
| --- | --- | --- |
| `usr/lib/libx264.so.164` | `usr/share/doc/libx264-164/copyright` | GPL-2 |
| `usr/lib/libx265.so.199` | `usr/share/doc/libx265-199/copyright` | GPL-2 and LGPL-2.1 |
| `usr/lib/libavcodec.so.59`, `libavformat.so.59`, `libavutil.so.57`, plus `gstreamer-1.0/libgstlibav.so` | FFmpeg package copyright files | Refer to GPL-2, GPL-3 and LGPL-2.1 terms |
| GStreamer core/runtime libraries | `usr/share/doc/libgstreamer1.0-0/copyright` | Refers to the system LGPL text |

The package copyright files describe whole source packages, so their licence stanzas alone do not establish which individual source files were compiled into each library. A final inventory needs the actual package/source versions and a source-availability plan for the redistributed libraries, as well as the missing full terms. Inspect the selected final AppImage again after packaging; this historical image is not a substitute for that check. The ARM .deb does not bundle these platform libraries; its package-manager dependencies are a different distribution boundary.

## Recovered sherpa native-source provenance

These expected source inputs come from sherpa-onnx v2.0.0's own CMake dependency manifests. Each downloaded source archive matched the SHA-256 pinned there before its notices were read. This verifies source provenance, not reproducible equivalence between a prebuilt static archive and source.

| Component | Pinned source | Original files retained in the evidence artifact |
| --- | --- | --- |
| kaldi-decoder | [v0.3.0](https://github.com/k2-fsa/kaldi-decoder/tree/v0.3.0) | LICENSE |
| kaldi-native-fbank | [v1.22.4](https://github.com/csukuangfj/kaldi-native-fbank/tree/v1.22.4) | LICENSE |
| OpenFst fork | [v1.8.5-2026-07-09](https://github.com/csukuangfj/openfst/tree/v1.8.5-2026-07-09) | COPYING and AUTHORS |
| simple-sentencepiece | [v0.7](https://github.com/pkufool/simple-sentencepiece/tree/v0.7) | LICENSE |
| utf8proc | [v2.12.0](https://github.com/JuliaStrings/utf8proc/tree/v2.12.0) | LICENSE.md, including Unicode terms |
| KissFFT | [febd4caeed32e33ad8b2e0bb5ea77542c40f18ec](https://github.com/mborgerding/kissfft/tree/febd4caeed32e33ad8b2e0bb5ea77542c40f18ec) | COPYING, LICENSES/BSD-3-Clause and LICENSES/Unlicense |

## Release boundary

No production packaging, installed app, public release or licence-policy approval was changed by this inventory. The collected files are review evidence, not a finished distribution bundle. Before publishing binaries, assemble and verify the final notice bundle for each selected package format, including native components and redistributed platform libraries. Keep any unresolved source/notice obligations visible rather than reporting them as passed.

## Implemented notice bundle follow-up

The bounded Rust/static-native assembly is now tracked in `src-tauri/resources/notices/` and mapped into the app's `notices/` resources. It contains 477 package mappings, 318 deduplicated original/standard text files, the standard-library notice and the six recovered native-source sets. All 32 packages without standalone cached licence files now have original upstream declarations or source licence headers plus full terms for every declared choice. SPDX terms are pinned to license-list-data v3.27.0, revision `d46e94e2c78ceede1cfc63cfa0396472d2798d4c`; their original template placeholders are labelled and are not invented copyright claims. notdot's own licence remains Apache-2.0.

`scripts/distribution_notices.py --write` deterministically generates the index from retained inputs. Its offline validator and local-CI hook check the lock identity, original text hashes, coverage, safe paths and references. Six scoped regression cases passed, including tampered text, missing attribution/declared terms, changed lockfile and unsafe paths. No application build or live test was performed for this resource-only follow-up. Final package resource-copy verification remains part of the coordinator's next candidate build. The earlier table describes the audit before this assembly; Windows CRT/SDK and AppImage platform/source obligations remain separate public-distribution gates.


## Binary redistribution follow-up

The shipping boundary and exact procedures are now in [Binary licences](../distribution/BINARY-LICENCES.md). The existing internal Windows QA toolchain was inspected without running the app: cargo-xwin 0.23.1/xwin 0.10.0; cache installation record identifies VS 2022 CRT 14.44.17.14 and SDK 10.0.26100, while `crtversion.h` identifies 14.44.35220. No SDK/CRT licence or REDIST text was retained in that cache. These private cross-built QA binaries are excluded from public release inputs; public Windows release provenance belongs to the actual official Windows runner/toolset, not these internal artifacts. Microsoft's linked-library distribution guidance and applicable terms are linked in the shipping document. No toolchain entitlement was invented.

The AppImage media framework remains enabled. A new same-builder collector inventories ELF package provenance, retains original copyright files and complete common licence texts, downloads exact distro source packages with their build recipes, includes exact committed notdot source and locked Rust vendor sources, and hash-checks the six pinned native archives. Explicit independent ELF inputs require actual notice/source files. It emits a source companion rather than a speculative written offer. The source companion must be published beside the selected final AppImage; the historical QA image is not approved for public upload.

Focused validation: four synthetic ELF provenance regressions passed (RPATH table change, changed executable code, changed build ID and non-ELF). An actual builder x264 shared library was copied and RPATH-patched with patchelf: the semantic symbol/build-ID/allocated-section comparison passed while byte hashes differed. This caught and corrected patchelf's dynamic-symbol index remapping. Exact signed-repository x264 source `2:0.164.3095+gitbaee400-3` downloaded successfully (upstream archive SHA-256 `8b237e94b08c196a1da22f2f25875f10be4cff3648df4eeff21e00da8f683fc2`, Debian patch archive `4980a5595db7465c8035e872d31e0eedd98dea936c1601f1cc338433eaff96b7`); dpkg-source verified/extracted it and applied the recorded Debian patch. Existing 477-package notice validation still passes.

These checks validate the collector's critical mechanisms, not a complete selected AppImage/source pair. No final public AppImage was rebuilt or uploaded during this follow-up. Final package/source correspondence, extracted AppRun replacement compatibility and official Windows runner records must be checked on the actual publication artifacts; passing private ARM Debian/UI tests does not substitute for them.


## Completed AppImage/source-pair proof

Merged source `c5773c3e776436ff10a918eea8a494cc1d524662` now has an actual rebuilt normal AppImage and corresponding-source archive. AppImage SHA-256: `fca5a6c18a636a64ce1f1d826a7fd8525087cc6e2f26544bc3cc47ba28953819` (207,161,848 bytes). Source archive: `728f83826643df030d61339e1ad3285b8d94ada05e2c905f9b063a1c222e776c` (981,650,537 bytes). All 17 original common licence texts are present and hash-verified inside the image. The source archive binds the exact image/runtime/executable/revision and provides 183 exact distro source packages, 422 ELF and 43 data-file provenance records, original notices/full terms, source/build recipes, locked Rust sources and pinned native inputs. Settings XML from data-only packages is included and its compiled form was independently regenerated. The full [Linux validation report](2026-10-29-linux.md) records build, cache and native updater evidence.

Official launcher provenance was recovered rather than guessed: AppRun exactly matches its official asset at AppImageKit revision `5735cc5bed206497cddfbd2a75e1982c2606c35d`; the runtime matches its official asset at type2-runtime revision `fedfad51acc2860b2916a1eef32ba4a500ff31ef`, apart from defined packaging metadata slots. The actual upstream build log identifies musl 1.2.5, mimalloc 2.1.7, zlib 1.3.2 and zstd 1.5.6. Their original, hash-pinned notices are retained alongside the runtime source and pinned FUSE 3.15.0/squashfuse 0.5.2 sources/patch/build recipes. Relinking is supported by supplied source/build materials; no bit-identical compiler reproduction claim or requirement is made.

The private historical dirty-tree AppImage remains a different artifact. Its platform-source recovery helped validate the collector, but it was not relabelled with a guessed application revision. The final merged-source pair above replaces it as the concrete redistribution proof. The public release path uses actual official Windows-runner toolchain/redistributable provenance, keeping the internal xwin QA binaries outside public release inputs. Future selected binary builds must pass their own mandatory pairing/provenance checks; no public release or Actions enablement occurred during this work.
