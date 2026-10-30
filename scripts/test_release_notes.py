import unittest
from release_notes import release_notes


class ChangelogReleaseNotes(unittest.TestCase):
    def test_selects_only_the_requested_dated_entry(self):
        text = '# Changelog\n\n## Unreleased\n\nLater.\n\n## [0.1.0] - 2026-10-30\n\nFirst release.\n\n- Speak naturally.\n\n## [0.0.1] - 2026-10-01\n\nEarlier.\n'
        self.assertEqual(release_notes(text, '0.1.0'), 'First release.\n\n- Speak naturally.\n')

    def test_missing_duplicate_or_empty_release_fails(self):
        for text in ['', '## [0.1.0] - 2026-10-30\n', '## [0.1.0] - 2026-10-30\nOne.\n## [0.1.0] - 2026-10-30\nTwo.']:
            with self.subTest(text=text), self.assertRaises(ValueError):
                release_notes(text, '0.1.0')

    def test_rejects_nonstable_versions_and_development_log_length(self):
        for version in ['../notes', '0.1.0-rc.1']:
            with self.subTest(version=version), self.assertRaises(ValueError):
                release_notes('', version)
        with self.assertRaises(ValueError):
            release_notes('## [0.1.0] - 2026-10-30\n\n' + '- Detail.\n' * 8, '0.1.0')
