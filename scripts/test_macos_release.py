import io
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch
from verify_macos_release import verify_archive


class MacReleaseArchiveTests(unittest.TestCase):
    def make_archive(self, root, names):
        archive = root / 'notdot.app.tar.gz'
        with tarfile.open(archive, 'w:gz') as stream:
            for name in names:
                member = tarfile.TarInfo(name)
                member.size = 4
                stream.addfile(member, io.BytesIO(b'test'))
        return archive

    def test_archive_verifies_the_packaged_app_not_only_the_build_folder(self):
        with tempfile.TemporaryDirectory() as folder:
            archive = self.make_archive(Path(folder), ['notdot.app/Contents/Info.plist'])
            with patch('verify_macos_release.verify_bundle') as verify:
                verify_archive(archive, 'TEAM', '0.1.0')
                verify.assert_called_once()
                self.assertEqual(verify.call_args.args[0].name, 'notdot.app')
                self.assertEqual(verify.call_args.args[1:], ('TEAM', '0.1.0'))

    def test_extra_root_payload_is_rejected(self):
        with tempfile.TemporaryDirectory() as folder:
            archive = self.make_archive(Path(folder), ['notdot.app/Contents/Info.plist', 'extra'])
            with self.assertRaisesRegex(ValueError, 'exactly notdot.app'):
                verify_archive(archive, 'TEAM', '0.1.0')

    def test_parent_traversal_never_reaches_bundle_verification(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            archive = self.make_archive(root, ['../escape'])
            with patch('verify_macos_release.verify_bundle') as verify:
                with self.assertRaises(tarfile.OutsideDestinationError):
                    verify_archive(archive, 'TEAM', '0.1.0')
                verify.assert_not_called()
            self.assertFalse((root / 'escape').exists())


@unittest.skipUnless(__import__('sys').platform == 'darwin', 'native BSD tar metadata check')
class NativeMacArchiveCompatibilityTests(unittest.TestCase):
    def test_ticket_file_survives_without_installer_incompatible_appledouble_entries(self):
        import os
        import subprocess
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            app = root / 'notdot.app'
            (app / 'Contents').mkdir(parents=True)
            ticket = app / 'Contents/CodeResources'
            ticket.write_bytes(b'benign-regular-ticket-marker')
            # Real BSD tar would emit a root ._notdot.app without this policy.
            subprocess.run(['/usr/bin/xattr', '-w', 'com.notdot.fixture', 'benign-xattr', str(app)], check=True)
            archive = root / 'notdot.app.tar.gz'
            environment = dict(os.environ, COPYFILE_DISABLE='1')
            subprocess.run(['/usr/bin/tar', '-czf', str(archive), '-C', str(root), app.name], check=True, env=environment)
            with tarfile.open(archive) as package:
                self.assertFalse(any(part.startswith('._') for member in package.getmembers() for part in Path(member.name).parts))
            def verify(unpacked, team, version):
                self.assertEqual((unpacked / 'Contents/CodeResources').read_bytes(), ticket.read_bytes())
            with patch('verify_macos_release.verify_bundle', side_effect=verify):
                verify_archive(archive, 'TEAM', '0.1.0')
