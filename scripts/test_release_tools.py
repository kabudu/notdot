import hashlib
import io
import json
from pathlib import Path
import stat
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import zipfile
from install_release_tools import executable_bytes, prepare_tool, verify_path, VERSIONS


def zipped(name='trunk', symlink=False):
    output = io.BytesIO()
    with zipfile.ZipFile(output, 'w') as archive:
        item = zipfile.ZipInfo(name)
        if symlink:
            item.create_system = 3
            item.external_attr = (stat.S_IFLNK | 0o777) << 16
        archive.writestr(item, b'synthetic executable')
    return output.getvalue()


def entry(data, suffix='.zip'):
    return {'version': '0.22.1', 'url': 'https://github.com/trunk-rs/trunk/releases/download/v0.22.1/fixture' + suffix,
            'archive_bytes': len(data), 'sha256': hashlib.sha256(data).hexdigest(), 'executable': 'trunk'}


class ReleaseToolAcceptance(unittest.TestCase):
    def test_checksum_mismatch_rejects_before_execution(self):
        data = zipped(); descriptor = entry(data); descriptor['sha256'] = '0' * 64
        with tempfile.TemporaryDirectory() as directory, patch('install_release_tools.subprocess.check_output') as execute:
            with self.assertRaisesRegex(ValueError, 'SHA-256'):
                prepare_tool('trunk', descriptor, Path(directory), fetch=lambda url: data)
            execute.assert_not_called()
            self.assertFalse(list(Path(directory).iterdir()))

    def test_only_the_authenticated_expected_zip_or_tar_binary_is_read(self):
        self.assertEqual(executable_bytes(zipped(), entry(zipped())), b'synthetic executable')
        output = io.BytesIO()
        with tarfile.open(fileobj=output, mode='w:gz') as archive:
            item = tarfile.TarInfo('tools/trunk'); item.size = 4
            archive.addfile(item, io.BytesIO(b'tool'))
        data = output.getvalue()
        self.assertEqual(executable_bytes(data, entry(data, '.tgz')), b'tool')

    def test_symlink_traversal_or_missing_executable_never_gets_installed(self):
        for data in [zipped('../trunk'), zipped('/trunk'), zipped(symlink=True), zipped('other')]:
            with self.subTest(data=data[:10]), self.assertRaises(ValueError):
                executable_bytes(data, entry(data))

    def test_wrong_native_version_is_rejected(self):
        data = zipped()
        with tempfile.TemporaryDirectory() as directory, patch('install_release_tools.subprocess.check_output', return_value='trunk 9.9.9'):
            with self.assertRaisesRegex(ValueError, 'version'):
                prepare_tool('trunk', entry(data), Path(directory), fetch=lambda url: data)

    def test_nonofficial_origin_or_wrong_version_is_rejected_before_download(self):
        for update in [{'url': 'https://example.invalid/tool.zip'}, {'version': '0.22.2'}]:
            descriptor = dict(entry(zipped()), **update)
            with tempfile.TemporaryDirectory() as directory, patch('install_release_tools.download') as fetch:
                with self.assertRaises(ValueError):
                    prepare_tool('trunk', descriptor, Path(directory), fetch=fetch)
                fetch.assert_not_called()

    def test_path_discovery_requires_verified_directory_and_real_cargo_subcommand(self):
        destination = Path('/fixture/tools').resolve()
        with patch('install_release_tools.platform.system', return_value='Linux'), patch('install_release_tools.shutil.which', side_effect=lambda name: str(destination / name)), patch('install_release_tools.subprocess.check_output', side_effect=['trunk 0.22.1', 'tauri-cli 2.12.4']) as execute:
            verify_path(destination)
            self.assertEqual(execute.call_args.args[0], ['cargo', 'tauri', '--version'])
        with patch('install_release_tools.shutil.which', return_value='/untrusted/cache/trunk'):
            with self.assertRaisesRegex(ValueError, 'precede cached'):
                verify_path(destination)

    def test_all_three_release_hosts_have_exact_official_version_and_digest_pins(self):
        manifest = json.loads(Path(__file__).with_name('release_tools.json').read_text())
        self.assertEqual(set(manifest['platforms']), {'Windows-x86_64', 'Darwin-arm64', 'Linux-x86_64'})
        for host, tools in manifest['platforms'].items():
            self.assertEqual(set(tools), set(VERSIONS))
            for tool, descriptor in tools.items():
                self.assertEqual(descriptor['version'], VERSIONS[tool])
                self.assertRegex(descriptor['sha256'], '^[0-9a-f]{64}$')
                self.assertGreater(descriptor['archive_bytes'], 1000000)
                self.assertTrue(descriptor['url'].startswith('https://github.com/'))
