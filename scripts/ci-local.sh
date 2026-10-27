#!/bin/sh
set -eu
cargo fmt --all --check
cargo test -p notdot-core -p notdot-voice -p notdot --locked
cargo clippy -p notdot-core -p notdot-voice -p notdot --all-targets --locked -- -D warnings
cargo clippy -p notdot-orb --target wasm32-unknown-unknown --locked -- -D warnings
cargo clippy -p notdot-orb --target wasm32-unknown-unknown --features debug-ui --locked -- -D warnings
mkdir -p .local
rustc --edition 2024 --test crates/notdot-orb/src/interaction.rs -o .local/gesture-tests
.local/gesture-tests
rustc --edition 2024 --test crates/notdot-orb/src/visual_state.rs -o .local/visual-tests
.local/visual-tests
python3 -m unittest discover -s scripts -p 'test_*.py'
git diff --check
