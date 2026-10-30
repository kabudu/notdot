#!/bin/sh
set -eu
# Archive verification uses the maintained tarfile extraction filter.
NOTDOT_TEST_PYTHON="${NOTDOT_TEST_PYTHON:-python3}"
if ! "$NOTDOT_TEST_PYTHON" -c 'import tarfile; assert hasattr(tarfile, "data_filter")' 2>/dev/null; then
  NOTDOT_TEST_PYTHON=python3.12
fi
"$NOTDOT_TEST_PYTHON" -c 'import tarfile; assert hasattr(tarfile, "data_filter"), "Python 3.12+ is required for release checks"'
"$NOTDOT_TEST_PYTHON" scripts/distribution_notices.py
"$NOTDOT_TEST_PYTHON" scripts/check_voice_fixtures.py
cargo fmt --all --check
cargo test -p notdot-core -p notdot-voice -p notdot-wake-runtime -p notdot --locked
cargo clippy -p notdot-core -p notdot-voice -p notdot-wake-runtime -p notdot --all-targets --locked -- -D warnings
cargo clippy -p notdot-orb --target wasm32-unknown-unknown --locked -- -D warnings
cargo clippy -p notdot-orb --target wasm32-unknown-unknown --features debug-ui --locked -- -D warnings
"$NOTDOT_TEST_PYTHON" -c "from pathlib import Path; s=Path('index.html').read_text(); assert 'composer' not in s and '<textarea' not in s"
mkdir -p .local
rustc --edition 2024 --test crates/notdot-orb/src/interaction.rs -o .local/gesture-tests
.local/gesture-tests
rustc --edition 2024 --test crates/notdot-orb/src/visual_state.rs -o .local/visual-tests
.local/visual-tests
rustc --edition 2024 --test crates/notdot-orb/src/speech_queue.rs -o .local/speech-queue-tests
.local/speech-queue-tests
"$NOTDOT_TEST_PYTHON" -m unittest discover -s scripts -p 'test_*.py'
git diff --check
