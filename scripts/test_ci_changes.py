from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
from ci_changes import classify, requires_full, validate_gate, PLATFORMS


def change(path, status='M', old='100644', new='100644'):
    return f':{old} {new} {"a" * 40} {"b" * 40} {status}\0{path}\0'.encode()


def results(full):
    return {'changes': {'result': 'success', 'outputs': {'full': full}},
            'content': {'result': 'success'},
            **{job: {'result': 'success' if full == 'true' else 'skipped'} for job in PLATFORMS}}


class ChangePolicyTests(unittest.TestCase):
    def test_known_copy_and_website_assets_take_the_fast_path(self):
        for path in ['README.md', 'CHANGELOG.md', 'CONTRIBUTING.md', 'SECURITY.md', 'CODE_OF_CONDUCT.md', 'ROADMAP.md', 'docs/guide.md', 'docs/index.html', 'docs/site-assets/site.css', 'docs/site-assets/orb.svg', 'docs/site-assets/demo/notdot-demo.mp4', 'docs/site-assets/demo/captions.en.vtt']:
            with self.subTest(path=path):
                self.assertFalse(requires_full(change(path)))
                self.assertFalse(requires_full(change(path, 'A', old='000000')))

    def test_unknown_build_dependency_and_release_inputs_require_full(self):
        for path in ['src-tauri/src/main.rs', 'Cargo.lock', 'Cargo.toml', '.cargo/config.toml', 'index.html', 'assets/updater.pub', 'scripts/check_site.py', '.github/workflows/ci.yml', 'LICENSE', 'docs/releases/0.1.0.md', 'docs/release-notes.md', 'docs//release-notes.md', 'docs/./release-notes.md', 'docs/validation/results.json', 'docs/site-assets/config.toml', '../README.md', 'docs/../README.md', 'docs/unsafe\\name.md']:
            with self.subTest(path=path):
                self.assertTrue(requires_full(change(path)))

    def test_deleted_renamed_symlink_executable_and_empty_diffs_require_full(self):
        for raw in [b'', change('README.md', 'D', new='000000'), change('docs/old.md', 'D', new='000000') + change('docs/new.md', 'A', old='000000'), change('README.md', 'T', new='120000'), change('docs/new.md', 'A', old='000000', new='120000'), change('docs/site-assets/site.js', new='100755')]:
            with self.subTest(raw=raw):
                self.assertTrue(requires_full(raw))
        for raw in [b'unknown\0file\0', change('README.md')[:-1], b'\xff\0']:
            with self.subTest(raw=raw), self.assertRaises(ValueError):
                requires_full(raw)

    def test_release_and_unavailable_metadata_cannot_select_the_fast_path(self):
        with patch('ci_changes.subprocess.check_output', side_effect=AssertionError('must not diff')):
            self.assertTrue(classify('workflow_dispatch', '', '', True))
            self.assertTrue(classify('pull_request', '', 'a' * 40))
            self.assertTrue(classify('push', '0' * 40, 'a' * 40))
        with patch('ci_changes.subprocess.check_output', side_effect=subprocess.CalledProcessError(1, ['git'])):
            with self.assertRaises(subprocess.CalledProcessError):
                classify('pull_request', 'a' * 40, 'b' * 40)

    def test_real_git_diff_detects_a_copy_change_and_a_rename_without_rename_detection(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            def git(*args):
                return subprocess.check_output(['git', '-c', 'commit.gpgsign=false', '-c', 'core.hooksPath=/dev/null', '-C', directory, *args], stderr=subprocess.DEVNULL).decode().strip()
            git('init'); git('config', 'user.name', 'CI fixture'); git('config', 'user.email', 'fixture@example.invalid')
            (root / 'README.md').write_text('First\n'); git('add', '.'); git('commit', '-m', 'baseline')
            base = git('rev-parse', 'HEAD')
            (root / 'README.md').write_text('Copy update\n'); git('add', '.'); git('commit', '-m', 'copy')
            copied = git('rev-parse', 'HEAD')
            with patch('ci_changes.subprocess.check_output', wraps=lambda args: subprocess.run(args, cwd=directory, check=True, capture_output=True).stdout):
                self.assertFalse(classify('pull_request', base, copied))
            (root / 'README.md').rename(root / 'CONTRIBUTING.md'); git('add', '-A'); git('commit', '-m', 'rename')
            renamed = git('rev-parse', 'HEAD')
            with patch('ci_changes.subprocess.check_output', wraps=lambda args: subprocess.run(args, cwd=directory, check=True, capture_output=True).stdout):
                self.assertTrue(classify('pull_request', copied, renamed))


class RequiredGateTests(unittest.TestCase):
    def test_both_selected_paths_are_accepted(self):
        validate_gate(results('true')); validate_gate(results('false'))

    def test_failed_or_missing_classification_and_content_always_fail(self):
        for update in [{'result': 'failure', 'outputs': {'full': 'true'}}, {'result': 'success', 'outputs': {}}, {'result': 'skipped', 'outputs': {}}, {'result': 'success', 'outputs': {'full': 'unknown'}}]:
            needs = results('false'); needs['changes'] = update
            with self.subTest(update=update), self.assertRaises(ValueError):
                validate_gate(needs)
        needs = results('false'); needs['content']['result'] = 'failure'
        with self.assertRaises(ValueError): validate_gate(needs)

    def test_each_native_failure_cancellation_or_unexpected_skip_is_blocking(self):
        for job in PLATFORMS:
            for status in ['failure', 'cancelled', 'skipped']:
                needs = results('true'); needs[job]['result'] = status
                with self.subTest(job=job, status=status), self.assertRaises(ValueError): validate_gate(needs)
            needs = results('false'); needs[job]['result'] = 'success'
            with self.assertRaises(ValueError): validate_gate(needs)
