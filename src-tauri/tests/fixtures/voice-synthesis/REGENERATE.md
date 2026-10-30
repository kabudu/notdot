# Regenerate the five fixed clips

Use Linux arm64, Piper release `2023.11.14-2`, and Python 3.9–3.12 (`audioop` is required). Download the archive and model from the immutable URLs in `../VOICE-FIXTURES.json`; verify both SHA-256 values before extraction/use. Tool/model downloads stay outside the tracked tree. The synthesis binary and its dependent libraries must remain together in the extracted `piper` directory.

The original generation ran in a Linux arm64 container with networking disabled, a two-CPU/two-GiB limit, and only the synthesis workspace mounted. No microphone, audio device, home directory or credentials were mounted. The release's macOS-aarch64 archive was not used.

The manifest contains the exact IPA strings supplied to Piper. They came from the release's `piper_phonemize --language en --espeak_data <piper-directory>/espeak-ng-data`; the name `/ˈɑːvə/` was corrected to `/ˈeɪvə/`. Replaying the saved strings avoids dictionary variation. The retained model configuration differs from the original only in `phoneme_type: text`.

Run this from the repository root after setting `PIPER_BIN` to the extracted executable and `PIPER_MODEL` to the verified `.onnx` file. It replaces only these five test WAVs. No playback occurs.

```python
import audioop
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import wave

fixtures = Path("src-tauri/tests/fixtures").resolve()
manifest = json.loads((fixtures / "VOICE-FIXTURES.json").read_text())
model = Path(os.environ["PIPER_MODEL"]).resolve()
assert hashlib.sha256(model.read_bytes()).hexdigest() == manifest["model"]["sha256"]
config = fixtures / manifest["generation"]["config"]
assert hashlib.sha256(config.read_bytes()).hexdigest() == manifest["generation"]["config_sha256"]
for row in manifest["files"]:
    with tempfile.TemporaryDirectory() as temp:
        raw = Path(temp) / "raw.wav"
        subprocess.run([
            os.environ["PIPER_BIN"], "--model", str(model),
            "--config", str(config), "--output_file", str(raw),
            "--length_scale", str(row["length_scale"]),
            "--noise_scale", "0", "--noise_w", "0",
            "--sentence_silence", "0.2",
        ], input=row["synthesis_phonemes"] + "\n", text=True, check=True)
        with wave.open(str(raw), "rb") as source:
            assert (source.getnchannels(), source.getsampwidth(), source.getframerate()) == (1, 2, 22050)
            pcm, _ = audioop.ratecv(source.readframes(source.getnframes()), 2, 1, 22050, 16000, None)
        pcm = b"\0\0" * 2400 + pcm + b"\0\0" * 4800
        output = fixtures / row["file"]
        with wave.open(str(output), "wb") as target:
            target.setparams((1, 2, 16000, 0, "NONE", "not compressed"))
            target.writeframes(pcm)
        assert hashlib.sha256(output.read_bytes()).hexdigest() == row["sha256"], output
```

Fixed zero noise controls were used to make the neural rendering repeatable. CPU/runtime differences can still change floating-point inference and final bytes: a mismatch must be reviewed, not silently accepted by updating hashes. Do not change recognition expectations, add gain, or tune padding to conceal a failed corpus test.
