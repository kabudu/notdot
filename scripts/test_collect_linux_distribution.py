#!/usr/bin/env python3
import importlib.util
from pathlib import Path
import struct
import tempfile
import tarfile
import io
from unittest.mock import patch
import prepare_appimage_source_inputs as producer
import prepare_linux_binary_notices as notice_preparer
import unittest

spec = importlib.util.spec_from_file_location('collector', Path(__file__).with_name('collect_linux_distribution.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def elf(text=b'code', dynamic=b'oldpath', build_id=b'build-id'):
    names = b'\0.text\0.dynamic\0.note.gnu.build-id\0.shstrtab\0'
    values = [(1, 1, 6, text), (7, 6, 3, dynamic), (16, 7, 2, build_id), (35, 3, 0, names)]
    header = bytearray(64)
    header[:6] = b'\x7fELF\x02\x01'
    sections = [bytes(64)]
    payload = bytearray()
    for name, kind, flags, data in values:
        sections.append(struct.pack('<IIQQQQIIQQ', name, kind, flags, 0, 64 + len(payload), len(data), 0, 0, 1, 0))
        payload.extend(data)
    struct.pack_into('<Q', header, 40, 64 + len(payload))
    struct.pack_into('<HHH', header, 58, 64, 5, 4)
    return bytes(header) + payload + b''.join(sections)


class Sections(unittest.TestCase):
    def compare(self, a, b):
        with tempfile.TemporaryDirectory() as directory:
            left, right = Path(directory) / 'a', Path(directory) / 'b'
            left.write_bytes(a)
            right.write_bytes(b)
            return module.load_sections(left), module.load_sections(right)

    def test_rpath_table_change_is_explicitly_tolerated(self):
        a, b = self.compare(elf(), elf(dynamic=b'new-long-path'))
        self.assertIsNotNone(a)
        self.assertEqual(a, b)

    def test_changed_program_code_rejected(self):
        a, b = self.compare(elf(), elf(text=b'evil'))
        self.assertNotEqual(a, b)

    def test_changed_build_id_rejected(self):
        a, b = self.compare(elf(), elf(build_id=b'other-id'))
        self.assertNotEqual(a, b)

    def test_non_elf_is_not_provenance(self):
        a, _ = self.compare(b'not ELF', elf())
        self.assertIsNone(a)


class Inputs(unittest.TestCase):
    def test_escaping_and_dangling_symlinks_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / 'app'
            root.mkdir()
            link = root / 'bad'
            link.symlink_to('../outside')
            with self.assertRaises(ValueError):
                module.check_symlinks(root)
            (Path(directory) / 'outside').write_text('outside')
            with self.assertRaises(ValueError):
                module.check_symlinks(root)

    def test_internal_file_and_directory_links_covered(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'lib').mkdir()
            (root / 'lib/code').write_text('code')
            (root / 'file').symlink_to('lib/code')
            (root / 'dir').symlink_to('lib', target_is_directory=True)
            module.check_symlinks(root)

    def test_empty_plaintext_and_symlink_source_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            notice = root / 'NOTICE'
            notice.write_text('Original notice')
            source = root / 'source.tar'
            source.write_text('not an archive')
            record = {'notice': str(notice), 'source': str(source)}
            with self.assertRaises(ValueError):
                module.copy_independent(record, root / 'output', 'a' * 64)
            source.write_bytes(b'')
            with self.assertRaises(ValueError):
                module.copy_independent(record, root / 'output', 'a' * 64)
            source.unlink()
            source.symlink_to(notice)
            with self.assertRaises(ValueError):
                module.copy_independent(record, root / 'output', 'a' * 64)

    def test_nonempty_source_archive_and_notice_copied(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            notice, source = root / 'NOTICE', root / 'source.tar'
            notice.write_text('Original notice')
            with tarfile.open(source, 'w') as archive:
                member = tarfile.TarInfo('source.c')
                member.size = 4
                archive.addfile(member, io.BytesIO(b'code'))
            module.copy_independent({'notice': str(notice), 'source': str(source)}, root / 'output', 'a' * 64)
            self.assertEqual(len(list((root / 'output').rglob('*.*'))), 1)

    def test_missing_appdir_rejected_before_any_collection(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaisesRegex(ValueError, 'AppDir'):
                module.collect(root / 'missing', root / 'out', {}, root, root / 'image', 1, 'a' * 64)
            self.assertFalse((root / 'out').exists())


class RuntimeBinding(unittest.TestCase):
    def test_only_defined_metadata_slot_may_differ(self):
        original = b'original-code' + bytes(4) + b'more-code'
        actual = b'original-code' + b'MD5!' + b'more-code'
        with patch.object(producer, 'sections', return_value={'.digest_md5': (13, 4)}):
            self.assertTrue(producer.runtime_equivalent(actual, original))
            self.assertFalse(producer.runtime_equivalent(b'X' + actual[1:], original))
            self.assertFalse(producer.runtime_equivalent(actual + b'X', original))

    def test_moved_metadata_slot_rejected(self):
        with patch.object(producer, 'sections', side_effect=[{'.digest_md5': (1, 4)}, {'.digest_md5': (2, 4)}]):
            self.assertFalse(producer.runtime_equivalent(b'0123456789', b'0123456789'))


class VerifiedCache(unittest.TestCase):
    def fixture(self, root):
        path = root / 'sources/example'
        path.mkdir(parents=True)
        (path / 'example.dsc').write_text('source-control')
        (path / 'example.tar.gz').write_bytes(b'archive fixture bytes')
        return {'root': root, 'manifest': {'sources': [{'package': 'example', 'version': '1.0'}],
                'files': {p.relative_to(root).as_posix(): module.sha(p) for p in path.iterdir()}}}

    def test_exact_version_cache_copies_verified_archives(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cache = self.fixture(root)
            output = root / 'output'
            output.mkdir()
            self.assertTrue(module.copy_cached_source(cache, output, 'example', '1.0'))
            self.assertEqual((output / 'example.dsc').read_text(), 'source-control')
            self.assertEqual(len(list(output.iterdir())), 2)
            self.assertFalse(module.copy_cached_source(cache, output, 'example', '2.0'))

    def test_tampered_cache_is_rejected_without_copying(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cache = self.fixture(root)
            (root / 'sources/example/example.tar.gz').write_bytes(b'tampered')
            output = root / 'output'
            output.mkdir()
            with self.assertRaisesRegex(ValueError, 'digest mismatch'):
                module.copy_cached_source(cache, output, 'example', '1.0')
            self.assertEqual(list(output.iterdir()), [])


class BinaryNoticePreparation(unittest.TestCase):
    def test_original_notice_copy_is_idempotent_and_hash_indexed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            docs = root / 'docs'
            (docs / 'library').mkdir(parents=True)
            (docs / 'library/copyright').write_bytes(b'Original copyright and licence')
            output = root / 'out'
            self.assertEqual(notice_preparer.prepare(output, docs), 1)
            self.assertEqual((output / 'library/copyright').read_bytes(), b'Original copyright and licence')
            self.assertEqual(notice_preparer.prepare(output, docs), 1)

    def test_unmarked_or_symlink_output_is_not_replaced(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / 'out'
            output.mkdir()
            sentinel = output / 'user-data'
            sentinel.write_text('keep')
            with self.assertRaisesRegex(ValueError, 'unmarked'):
                notice_preparer.prepare(output, root)
            self.assertEqual(sentinel.read_text(), 'keep')
            link = root / 'linked'
            link.symlink_to(output, target_is_directory=True)
            with self.assertRaisesRegex(ValueError, 'symlinked'):
                notice_preparer.prepare(link, root)


class RuntimeNoticeGate(unittest.TestCase):
    def fixture(self, root):
        project, app, inputs = root / 'project', root / 'app', root / 'inputs'
        for p in (project / 'third_party/appimage-runtime', app / 'usr/share/doc/notdot/appimage-runtime', inputs / 'runtime-x86_64'):
            p.mkdir(parents=True)
        original = b'Original runtime notice'
        digest = __import__('hashlib').sha256(original).hexdigest()
        for p in (project / 'third_party/appimage-runtime/NOTICE', app / 'usr/share/doc/notdot/appimage-runtime/NOTICE', inputs / 'runtime-x86_64/NOTICE'):
            p.write_bytes(original)
        (inputs / 'runtime-x86_64/release.json').write_text(__import__('json').dumps({'target_commitish': 'a' * 40}))
        (project / 'third_party/appimage-runtime.json').write_text(__import__('json').dumps({'files': {'NOTICE': {'sha256': digest, 'origin': {'component_file': 'runtime-x86_64/NOTICE', 'revision': 'a' * 40}}}}))
        return project, app, inputs

    def test_missing_or_tampered_shipped_notice_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            project, app, inputs = self.fixture(Path(directory))
            producer.validate_binary_notices(project, app, inputs)
            notice = app / 'usr/share/doc/notdot/appimage-runtime/NOTICE'
            notice.write_bytes(b'changed')
            with self.assertRaisesRegex(ValueError, 'differs'):
                producer.validate_binary_notices(project, app, inputs)
            notice.unlink()
            with self.assertRaises(FileNotFoundError):
                producer.validate_binary_notices(project, app, inputs)

    def test_different_original_source_or_revision_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            project, app, inputs = self.fixture(Path(directory))
            (inputs / 'runtime-x86_64/NOTICE').write_bytes(b'changed upstream notice')
            with self.assertRaisesRegex(ValueError, 'upstream source'):
                producer.validate_binary_notices(project, app, inputs)
            (inputs / 'runtime-x86_64/release.json').write_text(__import__('json').dumps({'target_commitish': 'b' * 40}))
            with self.assertRaisesRegex(ValueError, 'revision mismatch'):
                producer.validate_binary_notices(project, app, inputs)


if __name__ == '__main__':
    unittest.main()
