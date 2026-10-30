import json
import os
from pathlib import Path
import subprocess
import tempfile
import traceback
import unittest
from unittest.mock import patch
from finalize_macos_release import finalize


class MacNotarizationFailureTests(unittest.TestCase):
    def run_failure(self, timeout):
        with tempfile.TemporaryDirectory() as folder:
            bundle = Path(folder)
            app = bundle / 'macos/notdot.app'
            app.mkdir(parents=True)
            (app / 'fixture').write_text('fixture')
            (bundle / 'dmg').mkdir()
            (bundle / 'dmg/notdot.dmg').write_bytes(b'fixture')
            calls = []
            def command(args, **kwargs):
                calls.append(args)
                if 'notarytool' in args:
                    if timeout:
                        raise subprocess.TimeoutExpired(args, 1)
                    return subprocess.CompletedProcess(args, 0, json.dumps({'status': 'Invalid'}), '')
                return subprocess.CompletedProcess(args, 0, '', '')
            env = {'APPLE_TEAM_ID': 'TEAM', 'APPLE_ID': 'fixture-user', 'APPLE_PASSWORD': 'fixture-private-password'}
            with patch.dict(os.environ, env), patch('finalize_macos_release.verify_bundle'), patch('finalize_macos_release.verify_archive'), patch('finalize_macos_release.verify_signer'), patch('finalize_macos_release.bundle_fingerprint', return_value=['fixture', 'seal']), patch('finalize_macos_release.subprocess.run', side_effect=command):
                try:
                    finalize(bundle, '0.1.0')
                    self.fail('Notarization failure must reject the distribution')
                except RuntimeError:
                    rendered = traceback.format_exc()
            self.assertNotIn('fixture-private-password', rendered)
            self.assertNotIn('fixture-user', rendered)
            self.assertFalse(any(args[0] == 'hdiutil' for args in calls))

    def test_timeout_never_prints_credential_arguments_or_mounts(self):
        self.run_failure(True)

    def test_rejected_submission_never_mounts_the_image(self):
        self.run_failure(False)

    def test_different_mounted_app_is_rejected_and_detached(self):
        with tempfile.TemporaryDirectory() as folder:
            bundle = Path(folder)
            (bundle / 'macos/notdot.app').mkdir(parents=True)
            (bundle / 'dmg').mkdir()
            (bundle / 'dmg/notdot.dmg').write_bytes(b'fixture')
            calls = []
            def command(args, **kwargs):
                calls.append(args)
                return subprocess.CompletedProcess(args, 0, json.dumps({'status': 'Accepted'}), '')
            env = {'APPLE_TEAM_ID': 'TEAM', 'APPLE_ID': 'fixture-user', 'APPLE_PASSWORD': 'fixture-private-password'}
            with patch.dict(os.environ, env), patch('finalize_macos_release.verify_bundle'), patch('finalize_macos_release.verify_archive'), patch('finalize_macos_release.verify_signer'), patch('finalize_macos_release.bundle_fingerprint', side_effect=[['original', 'seal'], ['changed', 'seal']]), patch('finalize_macos_release.subprocess.run', side_effect=command):
                with self.assertRaisesRegex(ValueError, 'Mounted app differs'):
                    finalize(bundle, '0.1.0')
            self.assertTrue(any(args[:2] == ['hdiutil', 'detach'] for args in calls))
