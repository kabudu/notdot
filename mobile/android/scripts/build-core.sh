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
# The desktop's Whisper backup (whisper.cpp via CMake) on top of the spotter.
if [ -n "$features" ] && [ "${NOTDOT_WHISPER:-1}" = 1 ]; then features="--features whisper"; fi
# whisper-rs-sys 0.15 chooses BLAS from the build host, not the target: on a
# macOS host it links ggml-blas, which is not built for Android. An empty
# archive satisfies that link (nothing references it); Linux hosts skip this.
stub_flags=""
if [ "$(uname)" = Darwin ] && [ -n "$features" ]; then
  stubs="$root/target/mobile/android-stubs"
  mkdir -p "$stubs"
  [ -f "$stubs/libggml-blas.a" ] || "$bin/llvm-ar" rcs "$stubs/libggml-blas.a"
  stub_flags="-L native=$stubs"
fi
out="$here/build-core"
mkdir -p "$out/jniLibs/arm64-v8a" "$out/kotlin" "$out/host"
cd "$root/mobile/rust"
CARGO_TARGET_DIR="$root/target/mobile" \
ANDROID_NDK_HOME="$ndk" \
CMAKE_ANDROID_NDK="$ndk" CMAKE_ANDROID_ARCH_ABI=arm64-v8a CMAKE_SYSTEM_VERSION=$api \
CC_aarch64_linux_android="$bin/aarch64-linux-android$api-clang" \
CXX_aarch64_linux_android="$bin/aarch64-linux-android$api-clang++" \
AR_aarch64_linux_android="$bin/llvm-ar" \
BINDGEN_EXTRA_CLANG_ARGS_aarch64_linux_android="--sysroot=$bin/../sysroot" \
CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$bin/aarch64-linux-android$api-clang" \
CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS="$stub_flags" \
  cargo rustc --locked --lib --crate-type cdylib --target aarch64-linux-android $flags $features
cp "$root/target/mobile/aarch64-linux-android/$profile/libnotdot_mobile_core.so" "$out/jniLibs/arm64-v8a/"
if [ -n "$features" ]; then
  # sherpa-onnx ships Android as shared libraries; the core needs the C API,
  # which needs ONNX Runtime. Both are placed beside the core by its build.
  for lib in libsherpa-onnx-c-api.so libonnxruntime.so; do
    cp "$root/target/mobile/aarch64-linux-android/$profile/$lib" "$out/jniLibs/arm64-v8a/"
  done
fi
case "$features" in
  # whisper.cpp is C++ and links the NDK's shared C++ runtime.
  *whisper*) cp "$bin/../sysroot/usr/lib/aarch64-linux-android/libc++_shared.so" "$out/jniLibs/arm64-v8a/" ;;
  *) rm -f "$out/jniLibs/arm64-v8a/libc++_shared.so" ;;
esac
# Host library (same crate and features) for bindings and JVM unit tests.
CARGO_TARGET_DIR="$root/target/mobile" cargo build -q --locked --lib $features
host=$(ls "$root/target/mobile/debug/libnotdot_mobile_core.dylib" "$root/target/mobile/debug/libnotdot_mobile_core.so" 2>/dev/null | head -1)
cp "$host" "$out/host/"
cd "$root"
cargo run -q -p notdot-mobile-core --no-default-features --features bindgen --bin uniffi-bindgen -- \
  generate --library "$host" --language kotlin --out-dir "$out/kotlin"
echo "built $(ls "$out/jniLibs/arm64-v8a" | tr '\n' ' ')"
