notdot: licences and notices

notdot’s own source and original artwork use Apache-2.0. Third-party
components retain their own licences, preserved in this folder.

Open INDEX.md for package links and full terms. manifest.json records the
locked package versions, original source provenance and retained text hashes.
Original copyright lines are preserved. Standard licence templates are labelled
as standard terms; their placeholders are not invented copyright claims.

This bundle covers the locked Rust dependency graphs and the listed known
static native/model components. It includes all declared licence choices.

Separate public-distribution checks remain for Windows static CRT/SDK terms
and Linux AppImage platform-library provenance, full terms and source
availability. This notice bundle does not claim to settle those checks.

Maintainers can reproduce the index offline:
  python3 scripts/distribution_notices.py --write
Validate it before packaging:
  python3 scripts/distribution_notices.py

If Cargo.lock changes, update the inventory and original, hash-pinned licence
inputs first; do not just replace the lockfile checksum. Scripts do not guess
copyright holders or download/execute upstream code.
