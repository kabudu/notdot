#!/bin/sh
# Build the Rust core for Android (arm64-v8a: current emulators on Apple
# silicon and the target phones) plus the Kotlin bindings and a host
# library for JVM unit tests. Outputs go to mobile/android/build-core
# (ignored). Usage: mobile/android/scripts/build-core.sh [debug|release]
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
root=$(cd "$here/../.." && pwd)
profile=${1:-debug}
flags=""; [ "$profile" = release ] && flags=--release
sdk=${ANDROID_HOME:-$HOME/Library/Android/sdk}
ndk=${ANDROID_NDK_HOME:-$(ls -d "$sdk"/ndk/* | sort -V | tail -1)}
api=29
bin="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin"
[ -d "$bin" ] || bin="$ndk/toolchains/llvm/prebuilt/linux-x86_64/bin"
features=""
if [ "${NOTDOT_WAKE:-1}" = 1 ]; then features="--features wake"; fi
out="$here/build-core"
mkdir -p "$out/jniLibs/arm64-v8a" "$out/kotlin" "$out/host"
cd "$root/mobile/rust"
CARGO_TARGET_DIR="$root/target/mobile" \
CC_aarch64_linux_android="$bin/aarch64-linux-android$api-clang" \
AR_aarch64_linux_android="$bin/llvm-ar" \
CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$bin/aarch64-linux-android$api-clang" \
  cargo rustc --locked --lib --crate-type cdylib --target aarch64-linux-android $flags $features
cp "$root/target/mobile/aarch64-linux-android/$profile/libnotdot_mobile_core.so" "$out/jniLibs/arm64-v8a/"
if [ -n "$features" ]; then
  # sherpa-onnx ships Android as shared libraries; the core needs the C API,
  # which needs ONNX Runtime. Both are placed beside the core by its build.
  for lib in libsherpa-onnx-c-api.so libonnxruntime.so; do
    cp "$root/target/mobile/aarch64-linux-android/$profile/$lib" "$out/jniLibs/arm64-v8a/"
  done
fi
# Host library (same crate and features) for bindings and JVM unit tests.
CARGO_TARGET_DIR="$root/target/mobile" cargo build -q --locked --lib $features
host=$(ls "$root/target/mobile/debug/libnotdot_mobile_core.dylib" "$root/target/mobile/debug/libnotdot_mobile_core.so" 2>/dev/null | head -1)
cp "$host" "$out/host/"
cd "$root"
cargo run -q -p notdot-mobile-core --no-default-features --features bindgen --bin uniffi-bindgen -- \
  generate --library "$host" --language kotlin --out-dir "$out/kotlin"
echo "built $(ls "$out/jniLibs/arm64-v8a" | tr '\n' ' ')"
