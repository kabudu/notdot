#!/bin/sh
# Build the Rust core (static library) and its Swift bindings into
# mobile/ios/build. Called by the Xcode "Build Rust core" phase, or by hand:
#   mobile/ios/scripts/build-core.sh iphonesimulator Debug
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
root=$(cd "$here/../.." && pwd)
sdk=${1:-${PLATFORM_NAME:-iphonesimulator}}
configuration=${2:-${CONFIGURATION:-Debug}}
# Xcode runs scripts with a minimal PATH.
PATH="$HOME/.cargo/bin:$PATH"
case "$sdk" in
  iphonesimulator) target=aarch64-apple-ios-sim ;;
  iphoneos) target=aarch64-apple-ios ;;
  *) echo "unsupported SDK: $sdk" >&2; exit 1 ;;
esac
profile=debug
flags=""
if [ "$configuration" = Release ]; then profile=release; flags=--release; fi
features=""
if [ "${NOTDOT_WAKE:-1}" = 1 ]; then features="--features wake"; fi
# The desktop's Whisper backup (whisper.cpp via CMake) on top of the spotter.
if [ -n "$features" ] && [ "${NOTDOT_WHISPER:-1}" = 1 ]; then features="--features whisper"; fi
# whisper.cpp builds with CMake; find it before the environment is cleared.
cmake_bin=$(PATH="$PATH:/opt/homebrew/bin:/usr/local/bin" command -v cmake || true)
# Run cargo outside Xcode's build environment: its SDK, toolchain and
# platform variables confuse the C builds of Rust dependencies (ring).
clean() {
  env -i HOME="$HOME" PATH="$HOME/.cargo/bin:/usr/bin:/bin:/usr/sbin:/sbin" TMPDIR="${TMPDIR:-/tmp}" USER="${USER:-}" \
    CARGO_TARGET_DIR="$root/target/mobile" ${cmake_bin:+CMAKE="$cmake_bin"} "$@"
}
(cd "$root/mobile/rust" && clean IPHONEOS_DEPLOYMENT_TARGET=18.0 \
  cargo rustc --locked --lib --crate-type staticlib --target "$target" $flags $features)
mkdir -p "$here/build/lib/$sdk" "$here/build/generated"
cp "$root/target/mobile/$target/$profile/libnotdot_mobile_core.a" "$here/build/lib/$sdk/"
if [ -n "$features" ]; then
  # sherpa-onnx ships iOS as SherpaOnnxC.framework's binary (ONNX Runtime
  # linked in); its install name is @rpath/SherpaOnnxC.framework/SherpaOnnxC.
  fw="$here/build/lib/$sdk/SherpaOnnxC.framework"
  mkdir -p "$fw"
  cp "$root/target/mobile/$target/$profile/libsherpa-onnx-c-api.dylib" "$fw/SherpaOnnxC"
  cat > "$fw/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>SherpaOnnxC</string>
<key>CFBundleIdentifier</key><string>dev.notdot.mobile.prototype.sherpaonnx</string>
<key>CFBundlePackageType</key><string>FMWK</string>
<key>CFBundleShortVersionString</key><string>2.0.0</string>
<key>CFBundleVersion</key><string>1</string>
<key>MinimumOSVersion</key><string>13.0</string>
</dict></plist>
PLIST
fi
# Swift bindings come from a host build of the same library and features.
(cd "$root/mobile/rust" && clean cargo build -q --locked --lib $features)
(cd "$root" && clean cargo run -q -p notdot-mobile-core --no-default-features --features bindgen --bin uniffi-bindgen -- \
   generate --library "$root/target/mobile/debug/libnotdot_mobile_core.dylib" --language swift --out-dir "$here/build/generated")
mv -f "$here/build/generated/notdot_mobile_coreFFI.modulemap" "$here/build/generated/module.modulemap"
