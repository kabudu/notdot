from pathlib import Path
import tempfile
import unittest
from check_content_links import check_links


class LocalContentLinks(unittest.TestCase):
    def test_valid_local_links_and_code_examples(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve(); (root / 'docs').mkdir()
            (root / 'docs/guide.md').write_text('# Guide\n[Home](../README.md)\n')
            (root / 'README.md').write_text('[Guide](docs/guide.md) [Remote](https://example.invalid)\n```md\n[example](missing.md)\n```\n')
            self.assertEqual(check_links(root), 2)

    def test_missing_and_external_files_are_rejected(self):
        for link in ['missing.md', '../outside.md']:
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory).resolve(); (root / 'README.md').write_text(f'[Broken]({link})')
                with self.subTest(link=link), self.assertRaises(ValueError): check_links(root)
