#!/usr/bin/env python3
"""Report synthetic-fixture helper startup/response timing, without a voice provider."""
import array
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import threading
import time
import wave


def main():
    root = Path(__file__).resolve().parents[1]
    helper = root / 'src-tauri/resources/wake-accelerated/notdot-wake-helper.exe'
    model = root / 'src-tauri/resources/wake/ggml-tiny.en-q5_1.bin'
    phrase = b'Hey Ava'
    records = []
    with tempfile.TemporaryFile() as errors:
        env = {key: os.environ[key] for key in ('SystemRoot', 'WINDIR') if key in os.environ}
        process = subprocess.Popen([str(helper), '--model', str(model)], env=env,
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=errors)
        def stop():
            try:
                process.kill()
            except OSError:
                pass
        watchdog = threading.Timer(20, stop)
        watchdog.daemon = True
        watchdog.start()
        try:
            started = time.monotonic()
            process.stdin.write(b'NDWK0001' + struct.pack('<H', len(phrase)) + phrase)
            process.stdin.flush()
            ready = process.stdout.read(8)
            records.append({'ready': ready == b'NDWKRDY1', 'seconds': time.monotonic() - started})
            if ready == b'NDWKRDY1':
                for identifier, name in enumerate(('hey-ava.wav', 'hey-ava.wav', 'hey-ava-uk.wav'), 1):
                    with wave.open(str(root / 'src-tauri/tests/fixtures' / name)) as wav:
                        assert (wav.getnchannels(), wav.getsampwidth(), wav.getframerate()) == (1, 2, 16000)
                        values = struct.unpack('<' + 'h' * wav.getnframes(), wav.readframes(wav.getnframes()))
                    audio = array.array('f', [v / 32768 for v in values] + [0.] * 8000)
                    assert len(audio) <= 96512
                    if sys.byteorder != 'little':
                        audio.byteswap()
                    started = time.monotonic()
                    process.stdin.write(b'\x01' + struct.pack('<QI', identifier, len(audio)) + audio.tobytes())
                    process.stdin.flush()
                    response = process.stdout.read(9)
                    records.append({'fixture': name, 'seconds': time.monotonic() - started,
                                    'response_hex': response.hex()})
                    if len(response) != 9:
                        break
            process.stdin.close()
            process.wait(timeout=5)
        except (OSError, subprocess.TimeoutExpired) as error:
            records.append({'probe_error': type(error).__name__})
        finally:
            watchdog.cancel()
            stop()
            process.wait(timeout=5)
            watchdog.join(timeout=1)
        errors.seek(0)
        print(json.dumps({'records': records, 'exit_code': process.returncode,
                          'stderr_tail': errors.read()[-4096:].decode('utf-8', errors='replace')}, indent=2))


if __name__ == '__main__':
    main()
