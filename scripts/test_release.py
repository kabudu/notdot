import tempfile
import unittest
import json
import plistlib
import hashlib
import io
import tarfile
from pathlib import Path
from prepare_release import stage, PLATFORMS, linux_source_record

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
            windows_installer = root / 'input/windows-x86_64/app-setup.exe'
            toolset = {'revision': 'fixture-revision', 'installer_sha256': hashlib.sha256(windows_installer.read_bytes()).hexdigest(),
                       'visualStudioVersion': '17.fixture', 'staticRuntimeInputs': ['fixture'], 'nativeCompilerRecords': ['fixture']}
            (root / 'input/windows-x86_64/toolset.json').write_text(json.dumps(toolset))
            (root / 'input/windows-x86_64/Microsoft-VS2022-distributable-list.html').write_text('fixture terms')
            (root / 'input/darwin-universal/notdot.dmg').write_bytes(b'fixture dmg')
            (root / 'input/linux-x86_64/notdot.deb').write_bytes(b'fixture deb')
            appimage = root / 'input/linux-x86_64/app.AppImage'
            source_archive = root / 'input/linux-x86_64/notdot-corresponding-source.tar.gz'
            record = {'notdot_revision': 'fixture-revision', 'binaries': ['fixture'], 'sources': ['fixture'], 'files': {'fixture': 'hash'},
                      'appimage_sha256': hashlib.sha256(appimage.read_bytes()).hexdigest(), 'appimage_bytes': appimage.stat().st_size}
            def companion(value):
                payload = json.dumps(value).encode()
                with tarfile.open(source_archive, 'w:gz') as stream:
                    item = tarfile.TarInfo('notdot-corresponding-source/manifest.json')
                    item.size = len(payload)
                    stream.addfile(item, io.BytesIO(payload))
            with self.assertRaisesRegex(ValueError, 'corresponding-source companion'):
                stage('9.9.9', root / 'input', root / 'missing-source')
            companion(dict(record, appimage_sha256='wrong'))
            with self.assertRaisesRegex(ValueError, 'different AppImage'):
                stage('9.9.9', root / 'input', root / 'wrong-source')
            companion(record)
            with self.assertRaisesRegex(ValueError, 'different revision'):
                linux_source_record(source_archive, appimage, 'other-revision')
            with self.assertRaisesRegex(ValueError, 'different .*revision'):
                stage('9.9.9', root / 'input', root / 'wrong-revision', 'other-revision')
            manifest = stage('9.9.9', root / 'input', root / 'out', 'fixture-revision')
            self.assertTrue((root / 'out/notdot_9.9.9_linux-x86_64-corresponding-source.tar.gz').is_file())
            self.assertEqual(set(manifest['platforms']), {'darwin-aarch64', 'darwin-x86_64', 'windows-x86_64', 'linux-x86_64'})
            self.assertEqual(manifest['platforms']['darwin-aarch64'], manifest['platforms']['darwin-x86_64'])
            self.assertIn('darwin-universal.app.tar.gz', manifest['platforms']['darwin-x86_64']['url'])
            self.assertTrue((root / 'out/SHA256SUMS.txt').is_file())
            with self.assertRaises(ValueError):
                stage('9.9.10', root / 'input', root / 'wrong-version')
            (root / 'input/windows-x86_64/app-setup.exe').unlink()
            with self.assertRaises(ValueError):
                stage('9.9.9', root / 'input', root / 'incomplete')
            self.assertFalse((root / 'incomplete').exists())
