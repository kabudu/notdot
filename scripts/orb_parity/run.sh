#!/bin/sh
# Prepare a pixel-parity run in .local/orb-parity, then serve it:
#   scripts/orb_parity/run.sh && python3 -m http.server 8108 --bind 127.0.0.1 --directory .local/orb-parity
# Open /compare.html. Uses only local files; no network.
set -eu
root=$(cd "$(dirname "$0")/../.." && pwd)
out="$root/.local/orb-parity"
rm -rf "$out" && mkdir -p "$out"
cargo run -q -p notdot-orb-gen -- msl "$out/msl"
cargo run -q -p notdot-orb-gen -- verify "$out/msl/OrbFragment.metal" "$out/msl/OrbVertex.metal"
swift "$root/scripts/orb_parity/render_metal.swift" "$out/msl" "$root/crates/notdot-orb/fixtures/orb-contract.json" "$out/metal" "${ORB_PARITY_SIZE:-256}" "${ORB_PARITY_STRIDE:-3}"
cp "$root/crates/notdot-orb/src/orb.frag" "$root/scripts/orb_parity/compare.html" "$out/"
