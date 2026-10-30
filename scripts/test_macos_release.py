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
