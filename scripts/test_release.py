import tempfile
import unittest
import json
import plistlib
from pathlib import Path
from prepare_release import stage, PLATFORMS

class ReleaseTests(unittest.TestCase):
    def test_hardened_mac_bundle_declares_only_microphone_entitlement(self):
        native = Path(__file__).resolve().parent.parent / 'src-tauri'
        config = json.loads((native / 'tauri.conf.json').read_text())
        entitlements = plistlib.loads((native / config['bundle']['macOS']['entitlements']).read_bytes())
        self.assertEqual(entitlements, {'com.apple.security.device.audio-input': True})
        info = plistlib.loads((native / config['bundle']['macOS']['infoPlist']).read_bytes())
        self.assertTrue(info['NSMicrophoneUsageDescription'])

    def test_complete_manifest_and_missing_platform_rejection(self):
        fixture = Path(__file__).resolve().parent.parent / 'src-tauri/tests/fixtures'
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for platform, suffix in PLATFORMS.items():
                folder = root / 'input' / platform
                folder.mkdir(parents=True)
                (folder / ('app' + suffix)).write_bytes((fixture / 'update.txt').read_bytes())
                (folder / ('app' + suffix + '.sig')).write_text((fixture / 'update.txt.sig').read_text())
            manifest = stage('9.9.9', root / 'input', root / 'out')
            self.assertEqual(set(manifest['platforms']), set(PLATFORMS))
            self.assertTrue((root / 'out/SHA256SUMS.txt').is_file())
            with self.assertRaises(ValueError):
                stage('9.9.10', root / 'input', root / 'wrong-version')
            (root / 'input/windows-x86_64/app-setup.exe').unlink()
            with self.assertRaises(ValueError):
                stage('9.9.9', root / 'input', root / 'incomplete')
            self.assertFalse((root / 'incomplete').exists())
