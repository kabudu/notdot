#!/usr/bin/env python3
"""Prepare the local echo-cancellation probe page from licensed synthetic fixtures.

python3 scripts/prepare_aec_probe.py
python3 -m http.server 8109 --bind 127.0.0.1 --directory .local/aec-probe
target/debug/notdot --aec-probe 'http://127.0.0.1:8109/aec-probe.html?trials=4'
# Real person or a separate device instead of this computer's speakers:
#   ...aec-probe.html?trials=5&person=external   (press Start, follow the prompts)
# On macOS run it from a bundle with the app's identity so the existing
# microphone permission applies; system output is part of the echo reference.

Plays synthetic speech through the speakers and listens on the microphone.
Results print as one `AEC-PROBE {json}` line. Keep them under .local; never
commit recordings. Debug builds only.
"""
from pathlib import Path
import argparse
import shutil
import wave

root = Path(__file__).resolve().parents[1]
fixtures = root / 'src-tauri/tests/fixtures'


def frames(name):
    with wave.open(str(fixtures / name)) as w:
        assert w.getframerate() == 16000 and w.getnchannels() == 1 and w.getsampwidth() == 2
        return w.readframes(w.getnframes())


def silence(seconds):
    return b'\0\0' * int(16000 * seconds)


def write(path, parts):
    with wave.open(str(path), 'wb') as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(16000)
        w.writeframes(b''.join(parts))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dist', type=Path, default=root / '.local/aec-probe')
    dist = parser.parse_args().dist
    dist.mkdir(parents=True, exist_ok=True)
    for name in ('aec-probe.html', 'aec-probe-worklet.js'):
        shutil.copyfile(root / 'scripts/fixtures' / name, dist / name)
    speech = frames('negative.wav')
    # About ten seconds of ordinary reply speech with short sentence gaps.
    write(dist / 'reply.wav', [speech, silence(0.2)] * 4)
    # A reply that says the wake phrase as its own sentence.
    write(dist / 'reply-quoting.wav', [speech, silence(0.3), frames('hey-ava.wav'), silence(0.3), speech])
    print('Prepared', dist)


if __name__ == '__main__':
    main()
