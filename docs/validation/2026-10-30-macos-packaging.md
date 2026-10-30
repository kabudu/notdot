# macOS ticket and container packaging

The first public build received an accepted Apple notarization response, then failed release verification because its app had no stapled ticket. The pinned Tauri signing wrapper starts `stapler` but does not check its exit status. An accepted submission alone did not prove a distributable app.

The release finalizer now explicitly staples and validates the app with bounded retries before making either container. It creates the updater archive with native BSD tar, signs those exact bytes, then builds a fresh DMG from the verified app using `ditto`, an Applications link and `hdiutil`. The DMG is signed, notarized, stapled, validated and mounted read-only; its app must match the original executable and resource seal.

The signing identity is imported into a temporary CI keychain. Setup captures subprocess output and suppresses credential-bearing exceptions; the certificate file is removed immediately. An unconditional workflow cleanup restores the previous keychain search list and removes the temporary keychain. Tauri's automatic certificate import is disabled for that build. Local validation did not access signing credentials or import a real certificate.

## Archive compatibility

The notarization ticket is a regular `Contents/CodeResources` file, separate from `_CodeSignature/CodeResources`. Native tar runs with `COPYFILE_DISABLE=1`: this retains the regular ticket while suppressing AppleDouble sidecars. Those sidecars are incompatible with the pinned updater's first-path-component stripping, and its installer does not perform `dot_clean` restoration. The existing Python data-filter extraction, traversal protections and size limits remain unchanged.

Verified offline on the development Mac:

- A copy of the already signed and stapled Lascoria app was archived with this native-tar policy and extracted through Python's data filter. Its regular ticket was byte-identical; extracted stapler validation, strict/deep code-signature verification and Gatekeeper execution assessment all passed. The archive contained 74 members, zero AppleDouble sidecars and 103,058,093 bytes. The installed source app was neither modified nor launched. Evidence: ignored `.local/macos-ticket-roundtrip/summary.json`.
- The existing disposable native updater acceptance fixture used the same native archive policy with a benign source xattr. Actual pinned Tauri installation and LaunchServices restart passed from 9.9.8 to 9.9.9, including failed-install writer recovery, a maximum of one writer, preserved synthetic profile, duplicate rejection and complete process cleanup. The ephemeral fixture signing key was deleted. Evidence: ignored `.local/macos-update-acceptance/run-80ij5j0c/summary.json`.
- The Python suite passed **54 tests**, including native benign-ticket/archive compatibility, checked stapling/retry/order, temporary-keychain cleanup and credential-error redaction. `actionlint` and `git diff --check` passed.

The updater fixture exercises real native installer/process boundaries against a fake runtime. It does not test the actual notdot account, production update key, consent/download UI or a newly notarized notdot release. The next GitHub-hosted macOS signing build must prove the complete revised certificate, Apple-ticket and DMG flow before publication.
