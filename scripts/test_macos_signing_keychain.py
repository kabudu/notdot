import base64
import json
import os
from pathlib import Path
import subprocess
import tempfile
import traceback
import unittest
from unittest.mock import patch
from macos_signing_keychain import setup, cleanup


class TemporarySigningKeychainTests(unittest.TestCase):
    def test_import_uses_private_file_and_cleanup_restores_the_previous_search_list(self):
        with tempfile.TemporaryDirectory() as directory:
            folder = Path(directory) / 'signing'
            calls = []
            original = ['/Users/fixture/Library/Keychains/login.keychain-db', '/fixture/with spaces.keychain-db']
            def command(args, **kwargs):
                calls.append(args)
                if args[1:4] == ['list-keychains', '-d', 'user'] and len(args) == 4:
                    return subprocess.CompletedProcess(args, 0, '\n'.join(json.dumps(x) for x in original), '')
                if args[1] == 'create-keychain':
                    Path(args[-1]).write_bytes(b'not-a-real-keychain')
                if args[1] == 'import':
                    certificate = Path(args[2])
                    self.assertEqual(certificate.read_bytes(), b'fixture certificate')
                    self.assertEqual(certificate.stat().st_mode & 0o777, 0o600)
                    self.assertEqual(folder.stat().st_mode & 0o777, 0o700)
                if args[1] == 'delete-keychain':
                    Path(args[2]).unlink()
                return subprocess.CompletedProcess(args, 0, '', '')
            env = {'APPLE_CERTIFICATE': base64.b64encode(b'fixture certificate').decode(), 'APPLE_CERTIFICATE_PASSWORD': 'fixture-password'}
            with patch.dict(os.environ, env, clear=True), patch('macos_signing_keychain.subprocess.run', side_effect=command):
                setup(folder)
                self.assertFalse((folder / 'certificate.p12').exists())
                cleanup(folder)
            self.assertFalse(folder.exists())
            restore = ['security', 'list-keychains', '-d', 'user', '-s', *original]
            self.assertIn(restore, calls)
            self.assertLess(calls.index(restore), next(i for i, args in enumerate(calls) if args[1] == 'delete-keychain'))

    def test_failure_never_renders_password_arguments_and_removes_certificate(self):
        with tempfile.TemporaryDirectory() as directory:
            folder = Path(directory) / 'signing'
            def command(args, **kwargs):
                if args[1] == 'import':
                    raise subprocess.TimeoutExpired(args, 1)
                return subprocess.CompletedProcess(args, 0, '"/fixture/login.keychain-db"', '')
            env = {'APPLE_CERTIFICATE': base64.b64encode(b'fixture certificate').decode(), 'APPLE_CERTIFICATE_PASSWORD': 'private-fixture-password'}
            with patch.dict(os.environ, env, clear=True), patch('macos_signing_keychain.subprocess.run', side_effect=command):
                try:
                    setup(folder)
                    self.fail('Failed import must reject setup')
                except RuntimeError:
                    rendered = traceback.format_exc()
            self.assertNotIn('private-fixture-password', rendered)
            self.assertNotIn('TimeoutExpired', rendered)
            self.assertFalse(folder.exists())

    def test_cleanup_is_safe_when_the_job_failed_before_setup(self):
        with tempfile.TemporaryDirectory() as directory:
            with patch('macos_signing_keychain.subprocess.run') as command:
                cleanup(Path(directory) / 'not-created')
            command.assert_not_called()
