import json
import os
from pathlib import Path
import subprocess
import tempfile
import traceback
import unittest
from unittest.mock import patch
from finalize_macos_release import finalize, staple


class MacNotarizationFailureTests(unittest.TestCase):
    def run_failure(self, timeout):
        with tempfile.TemporaryDirectory() as folder:
            bundle = Path(folder)
            app = bundle / 'macos/notdot.app'
            app.mkdir(parents=True)
            (app / 'fixture').write_text('fixture')
            (bundle / 'dmg').mkdir()
            (bundle / 'dmg/notdot_0.1.0_universal.dmg').write_bytes(b'fixture')
            calls = []
            def command(args, **kwargs):
                calls.append(args)
                if 'notarytool' in args:
                    if timeout:
                        raise subprocess.TimeoutExpired(args, 1)
                    return subprocess.CompletedProcess(args, 0, json.dumps({'status': 'Invalid'}), '')
                return subprocess.CompletedProcess(args, 0, '', '')
            env = {'APPLE_TEAM_ID': 'TEAM', 'APPLE_SIGNING_IDENTITY': 'Developer ID Application: Fixture (TEAM)', 'APPLE_ID': 'fixture-user', 'APPLE_PASSWORD': 'fixture-private-password'}
            with patch.dict(os.environ, env), patch('finalize_macos_release.verify_bundle'), patch('finalize_macos_release.verify_archive'), patch('finalize_macos_release.verify_signer'), patch('finalize_macos_release.bundle_fingerprint', return_value=['fixture', 'seal']), patch('finalize_macos_release.subprocess.run', side_effect=command):
                try:
                    finalize(bundle, '0.1.0')
                    self.fail('Notarization failure must reject the distribution')
                except RuntimeError:
                    rendered = traceback.format_exc()
            self.assertNotIn('fixture-private-password', rendered)
            self.assertNotIn('fixture-user', rendered)
            self.assertFalse(any(args[:2] == ['hdiutil', 'attach'] for args in calls))

    def test_timeout_never_prints_credential_arguments_or_mounts(self):
        self.run_failure(True)

    def test_rejected_submission_never_mounts_the_image(self):
        self.run_failure(False)

    def test_different_mounted_app_is_rejected_and_detached(self):
        with tempfile.TemporaryDirectory() as folder:
            bundle = Path(folder)
            (bundle / 'macos/notdot.app').mkdir(parents=True)
            (bundle / 'dmg').mkdir()
            (bundle / 'dmg/notdot_0.1.0_universal.dmg').write_bytes(b'fixture')
            calls = []
            def command(args, **kwargs):
                calls.append(args)
                return subprocess.CompletedProcess(args, 0, json.dumps({'status': 'Accepted'}), '')
            env = {'APPLE_TEAM_ID': 'TEAM', 'APPLE_SIGNING_IDENTITY': 'Developer ID Application: Fixture (TEAM)', 'APPLE_ID': 'fixture-user', 'APPLE_PASSWORD': 'fixture-private-password'}
            with patch.dict(os.environ, env), patch('finalize_macos_release.verify_bundle'), patch('finalize_macos_release.verify_archive'), patch('finalize_macos_release.verify_signer'), patch('finalize_macos_release.bundle_fingerprint', side_effect=[['original', 'seal'], ['changed', 'seal']]), patch('finalize_macos_release.subprocess.run', side_effect=command):
                with self.assertRaisesRegex(ValueError, 'Mounted app differs'):
                    finalize(bundle, '0.1.0')
            self.assertTrue(any(args[:2] == ['hdiutil', 'detach'] for args in calls))


class ExplicitStaplingTests(unittest.TestCase):
    def test_staple_failure_is_checked_and_prevents_all_packaging(self):
        env = {'APPLE_TEAM_ID': 'TEAM', 'APPLE_SIGNING_IDENTITY': 'fixture'}
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, env):
            with patch('finalize_macos_release.subprocess.run', return_value=subprocess.CompletedProcess([], 65)) as command, patch('finalize_macos_release.time.sleep') as sleep, patch('finalize_macos_release.verify_bundle') as verify:
                with self.assertRaisesRegex(RuntimeError, 'staple and validate'):
                    finalize(Path(directory), '0.1.0')
            verify.assert_not_called()
            self.assertEqual(command.call_count, 5)
            self.assertEqual(sleep.call_count, 4)
            self.assertTrue(all(call.args[0][1:3] == ['stapler', 'staple'] for call in command.call_args_list))

    def test_ticket_propagation_is_bounded_and_validation_is_required(self):
        results = [subprocess.CompletedProcess([], 65), subprocess.CompletedProcess([], 0), subprocess.CompletedProcess([], 0)]
        with patch('finalize_macos_release.subprocess.run', side_effect=results) as command, patch('finalize_macos_release.time.sleep') as sleep:
            staple(Path('/fixture/notdot.app'))
        self.assertEqual(command.call_count, 3)
        self.assertEqual(command.call_args_list[-1].args[0][1:3], ['stapler', 'validate'])
        sleep.assert_called_once_with(15)


class MacContainerOrderingTests(unittest.TestCase):
    def test_both_containers_use_the_explicitly_stapled_app_and_exact_archive(self):
        with tempfile.TemporaryDirectory() as folder:
            bundle = Path(folder).resolve()
            app = bundle / 'macos/notdot.app'
            app.mkdir(parents=True)
            events = []
            def command(args, **kwargs):
                if args[0] == '/usr/bin/tar':
                    self.assertEqual(kwargs['env']['COPYFILE_DISABLE'], '1')
                    self.assertEqual(args[-2:], [str(app.parent), app.name])
                    Path(args[2]).write_bytes(b'exact-updater-bytes')
                    events.append('archive')
                if args[:4] == ['cargo', 'tauri', 'signer', 'sign']:
                    self.assertEqual(Path(args[-1]).read_bytes(), b'exact-updater-bytes')
                    events.append('signature')
                if args[0] == '/usr/bin/ditto':
                    self.assertEqual(args[1], str(app))
                    events.append('dmg-copy')
                if args[:2] == ['hdiutil', 'create']:
                    stage = Path(args[args.index('-srcfolder') + 1])
                    self.assertEqual((stage / 'Applications').readlink(), Path('/Applications'))
                return subprocess.CompletedProcess(args, 0, json.dumps({'status': 'Accepted'}), '')
            env = {'APPLE_TEAM_ID': 'TEAM', 'APPLE_SIGNING_IDENTITY': 'fixture', 'APPLE_ID': 'fixture-user', 'APPLE_PASSWORD': 'fixture-password'}
            with patch.dict(os.environ, env), patch('finalize_macos_release.staple', side_effect=lambda path: events.append('staple')), patch('finalize_macos_release.verify_bundle', side_effect=lambda *args: events.append('verify-bundle')), patch('finalize_macos_release.verify_archive', side_effect=lambda *args: events.append('verify-archive')), patch('finalize_macos_release.verify_signer'), patch('finalize_macos_release.bundle_fingerprint', return_value=['same-executable', 'same-seal']), patch('finalize_macos_release.subprocess.run', side_effect=command):
                finalize(bundle, '0.1.0')
            self.assertLess(events.index('staple'), events.index('verify-bundle'))
            self.assertLess(events.index('verify-bundle'), events.index('archive'))
            self.assertLess(events.index('archive'), events.index('signature'))
            self.assertLess(events.index('signature'), events.index('verify-archive'))
            self.assertLess(events.index('verify-archive'), events.index('dmg-copy'))
