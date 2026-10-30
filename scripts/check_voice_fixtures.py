#!/usr/bin/env python3
"""Check the public wake corpus and its retained licensing/source provenance."""
import hashlib
import json
from pathlib import Path
import wave


def check(root=Path('src-tauri/tests/fixtures')):
    record = json.loads((root / 'VOICE-FIXTURES.json').read_text(encoding='utf-8'))
    expected = {'hey-ava.wav', 'hey-ava-uk.wav', 'hey-ava-us.wav', 'ava.wav', 'negative.wav'}
    if {row['file'] for row in record['files']} != expected or len(record['files']) != 5 or {path.name for path in root.glob('*.wav')} != expected:
        raise ValueError('The five-file wake corpus must be inventoried exactly')
    if not record['fixture_license'].startswith('Apache-2.0') or record['tool']['license'] != 'MIT':
        raise ValueError('Review the public fixture licensing before distribution')
    for row in record['files']:
        path = root / row['file']
        if path.is_symlink() or not 0 < path.stat().st_size <= 512 * 1024:
            raise ValueError('Invalid wake fixture: ' + path.name)
        if hashlib.sha256(path.read_bytes()).hexdigest() != row['sha256']:
            raise ValueError('Wake fixture hash mismatch: ' + path.name)
        with wave.open(str(path), 'rb') as audio:
            if (audio.getnchannels(), audio.getsampwidth(), audio.getframerate(), audio.getnframes()) != (1, 2, 16000, row['frames']):
                raise ValueError('Wake fixture format mismatch: ' + path.name)
    source_names = {'PIPER-ENGINE-LICENSE.md', 'PIPER-VOICES-README.md', 'LJSPEECH-MODEL-CARD.md', 'REGENERATE.md', 'ljspeech-fixture.onnx.json'}
    if set(record['retained_source_files']) != source_names:
        raise ValueError('Missing original voice-fixture source/terms')
    for name, digest in record['retained_source_files'].items():
        if Path(name).name != name:
            raise ValueError('Fixture provenance must be local retained files')
        path = root / 'voice-synthesis' / name
        if path.is_symlink() or not 0 < path.stat().st_size <= 1024 * 1024 or hashlib.sha256(path.read_bytes()).hexdigest() != digest:
            raise ValueError('Fixture provenance mismatch: ' + name)
    print('Five licensed wake clips and retained synthesis provenance checked.')


if __name__ == '__main__':
    check()
