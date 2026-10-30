"""Integrity and coverage checks for retained binary attribution data."""
import copy
import json
from pathlib import Path
import shutil
import tempfile
import unittest

import distribution_notices as notices


class DistributionNotices(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bundle = self.root / 'notices'
        shutil.copytree(notices.BUNDLE, self.bundle)
        self.lock = self.root / 'Cargo.lock'
        shutil.copyfile(notices.ROOT / 'Cargo.lock', self.lock)

    def manifest(self):
        return json.loads((self.bundle / 'manifest.json').read_text())

    def save(self, manifest):
        (self.bundle / 'manifest.json').write_text(json.dumps(manifest))

    def test_current_inputs_generate_the_committed_index(self):
        manifest = notices.validate(self.bundle, self.lock)
        self.assertEqual(notices.render(manifest), (self.bundle / 'INDEX.md').read_text())
        config = json.loads((notices.ROOT / 'src-tauri/tauri.conf.json').read_text())
        self.assertEqual(config['bundle']['resources']['resources/notices/'], 'notices/')

    def test_tampered_original_text_is_rejected(self):
        item = next(iter(self.manifest()['texts'].values()))
        with (self.bundle / item['file']).open('ab') as f:
            f.write(b'changed')
        with self.assertRaisesRegex(ValueError, 'checksum mismatch'):
            notices.validate(self.bundle, self.lock)

    def test_missing_declared_licence_choice_is_rejected(self):
        manifest = self.manifest()
        package = next(p for p in manifest['packages'] if len(p['standard_terms']) > 1)
        package['standard_terms'].pop(next(iter(package['standard_terms'])))
        self.save(manifest)
        with self.assertRaisesRegex(ValueError, 'incomplete licence mapping'):
            notices.validate(self.bundle, self.lock)

    def test_original_attribution_cannot_be_replaced_by_only_standard_terms(self):
        manifest = self.manifest()
        manifest['packages'][0]['original_notices'] = []
        self.save(manifest)
        with self.assertRaisesRegex(ValueError, 'incomplete licence mapping'):
            notices.validate(self.bundle, self.lock)

    def test_changed_lockfile_is_rejected(self):
        self.lock.write_text('changed')
        with self.assertRaisesRegex(ValueError, 'Cargo.lock changed'):
            notices.validate(self.bundle, self.lock)

    def test_unsafe_text_path_is_rejected_before_reading_it(self):
        manifest = self.manifest()
        next(iter(manifest['texts'].values()))['file'] = '../outside'
        self.save(manifest)
        with self.assertRaisesRegex(ValueError, 'unsafe notice text path'):
            notices.validate(self.bundle, self.lock)


if __name__ == '__main__':
    unittest.main()
