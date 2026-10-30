from pathlib import Path
import shutil
import tempfile
import unittest
from check_voice_fixtures import check


class PublicVoiceFixtureTests(unittest.TestCase):
    def test_changed_audio_is_not_silently_accepted(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder) / 'fixtures'
            shutil.copytree(Path('src-tauri/tests/fixtures'), root)
            path = root / 'hey-ava.wav'
            data = bytearray(path.read_bytes())
            data[-1] ^= 1
            path.write_bytes(data)
            with self.assertRaisesRegex(ValueError, 'hash mismatch'):
                check(root)

    def test_missing_original_terms_fail_the_distribution_check(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder) / 'fixtures'
            shutil.copytree(Path('src-tauri/tests/fixtures'), root)
            (root / 'voice-synthesis/PIPER-ENGINE-LICENSE.md').unlink()
            with self.assertRaises((ValueError, FileNotFoundError)):
                check(root)
