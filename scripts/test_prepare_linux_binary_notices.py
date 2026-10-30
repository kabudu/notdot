import json
from pathlib import Path
import tempfile
import unittest
from prepare_linux_binary_notices import prepare


class PackageNoticeBoundaryTests(unittest.TestCase):
    def test_copyright_symlink_cannot_copy_a_private_file_into_the_bundle(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            documents = root / 'doc'
            (documents / 'package').mkdir(parents=True)
            private = root / 'private'
            private.write_text('fixture secret; never publish')
            (documents / 'package/copyright').symlink_to(private)
            with self.assertRaisesRegex(ValueError, 'escapes'):
                prepare(root / 'output', documents)
            self.assertFalse((root / 'output/package/copyright').exists())

    def test_original_package_aliases_inside_doc_remain_supported(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            documents = root / 'doc'
            (documents / 'original').mkdir(parents=True)
            (documents / 'alias').mkdir()
            original = documents / 'original/copyright'
            original.write_text('Original fixture package notice')
            (documents / 'alias/copyright').symlink_to('../original/copyright')
            self.assertEqual(prepare(root / 'output', documents), 2)
            self.assertEqual((root / 'output/alias/copyright').read_bytes(), original.read_bytes())
            record = json.loads((root / 'output/manifest.json').read_text())
            self.assertEqual(len(record['files']), 2)
