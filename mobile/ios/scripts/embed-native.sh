#!/bin/sh
# Xcode phase: embed SherpaOnnxC.framework and the desktop's wake models
# (keyword spotter files, plus the Whisper backup model when the core is
# built with it) into the app bundle. Nothing is copied into the source tree.
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
root=$(cd "$here/../.." && pwd)
frameworks="$TARGET_BUILD_DIR/$FRAMEWORKS_FOLDER_PATH"
mkdir -p "$frameworks"
rm -rf "$frameworks/SherpaOnnxC.framework"
cp -R "$here/build/lib/$PLATFORM_NAME/SherpaOnnxC.framework" "$frameworks/"
if [ "${CODE_SIGNING_ALLOWED:-NO}" = YES ] && [ -n "${EXPANDED_CODE_SIGN_IDENTITY:-}" ]; then
  codesign --force --sign "$EXPANDED_CODE_SIGN_IDENTITY" --timestamp=none "$frameworks/SherpaOnnxC.framework"
fi
models="$TARGET_BUILD_DIR/$UNLOCALIZED_RESOURCES_FOLDER_PATH/wake"
mkdir -p "$models"
src="$root/src-tauri/resources/wake"
cp "$src/bpe.model" "$src/encoder.onnx" "$src/decoder.onnx" "$src/joiner.onnx" "$src/tokens.txt" "$src/LICENSE" "$src/NOTICE.md" "$models/"
rm -rf "$models/phonetic" && cp -R "$src/phonetic" "$models/phonetic"
if [ "${NOTDOT_WHISPER:-1}" = 1 ]; then
  cp "$src/ggml-tiny.en-q5_1.bin" "$src/WHISPER-LICENSE" "$models/"
else
  rm -f "$models/ggml-tiny.en-q5_1.bin" "$models/WHISPER-LICENSE"
fi
