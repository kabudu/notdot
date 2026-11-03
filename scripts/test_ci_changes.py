from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
from ci_changes import classify, native_fingerprint, reusable_native_run, requires_full, validate_gate, PLATFORMS


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

        # Construct separators explicitly so source/log escaping cannot hide
        # whether the input contains one Windows separator or two.
        for separator in (chr(92), chr(92) * 2):
            with self.subTest(separator_length=len(separator)):
                self.assertTrue(requires_full(change('docs/file' + separator + 'guide.md')))

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


class NativeReuseTests(unittest.TestCase):
    def setUp(self):
        self.previous = 'a' * 40
        self.head = 'b' * 40
        self.fingerprint = 'c' * 64
        self.artifact = {'name': 'native-validation-' + self.fingerprint, 'expired': False,
                         'workflow_run': {'id': 42, 'head_sha': self.previous}}
        self.run = {'status': 'completed', 'conclusion': 'success', 'event': 'pull_request',
                    'path': '.github/workflows/ci.yml', 'head_sha': self.previous,
                    'head_repository': {'id': 123}}

    def reuse(self, artifacts=None, run=None, changes=None, ancestor=True):
        with patch.dict('os.environ', {'GITHUB_REPOSITORY': 'owner/repo', 'GITHUB_REPOSITORY_ID': '123'}), \
                patch('ci_changes.github_json', side_effect=[{'artifacts': artifacts if artifacts is not None else [self.artifact]}, run if run is not None else self.run]), \
                patch('ci_changes.subprocess.run', return_value=subprocess.CompletedProcess([], 0 if ancestor else 1)), \
                patch('ci_changes.subprocess.check_output', return_value=changes if changes is not None else change('README.md')):
            return reusable_native_run(self.fingerprint, self.head)

    def test_successful_ancestor_and_identical_tested_merge_tree_can_be_reused(self):
        self.assertEqual(self.reuse(), 42)

    def test_source_policy_release_and_mode_changes_cannot_reuse_evidence(self):
        for raw in [change('src-tauri/src/main.rs'), change('.github/workflows/ci.yml'),
                    change('scripts/ci_changes.py'), change('docs/releases/0.1.1.md'),
                    change('README.md', 'D', new='000000'), change('README.md', new='100755'), b'']:
            with self.subTest(raw=raw):
                self.assertIsNone(self.reuse(changes=raw))
        self.assertIsNone(self.reuse(ancestor=False))

    def test_missing_expired_wrong_tree_and_unsuccessful_evidence_fail_closed(self):
        self.assertIsNone(self.reuse(artifacts=[]))
        for update in [{'expired': True}, {'name': 'native-validation-' + 'd' * 64},
                       {'workflow_run': {'id': 42, 'head_sha': self.head}}]:
            self.assertIsNone(self.reuse(artifacts=[{**self.artifact, **update}]))
        for update in [{'status': 'in_progress'}, {'conclusion': 'failure'}, {'conclusion': 'cancelled'},
                       {'path': '.github/workflows/other.yml'}, {'event': 'workflow_dispatch'},
                       {'head_sha': self.head}, {'head_repository': {'id': 999}}]:
            with self.subTest(update=update):
                self.assertIsNone(self.reuse(run={**self.run, **update}))
        with patch.dict('os.environ', {'GITHUB_REPOSITORY': 'owner/repo'}), \
                patch('ci_changes.github_json', side_effect=OSError('offline')):
            self.assertIsNone(reusable_native_run(self.fingerprint, self.head))

    def test_real_tree_fingerprint_excludes_copy_but_includes_code_modes_and_base_inputs(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            def git(*args):
                return subprocess.run(['git', '-c', 'commit.gpgsign=false', '-c', 'core.hooksPath=/dev/null', '-C', directory, *args], check=True, capture_output=True).stdout
            def commit():
                git('add', '-A'); git('commit', '-m', 'fixture')
            def fingerprint():
                with patch('ci_changes.subprocess.check_output', wraps=lambda args: subprocess.run(args, cwd=directory, check=True, capture_output=True).stdout):
                    return native_fingerprint()
            git('init'); git('config', 'user.name', 'Fixture'); git('config', 'user.email', 'fixture@example.invalid')
            (root / 'README.md').write_text('First\n'); (root / 'source.rs').write_text('fn main() {}\n'); commit()
            original = fingerprint()
            (root / 'README.md').write_text('New copy\n'); commit()
            self.assertEqual(original, fingerprint())
            # The checked-out tree includes base-branch inputs too, rather than
            # hashing only the feature branch's changed files.
            (root / 'base-input.toml').write_text('changed = true\n'); commit()
            self.assertNotEqual(original, fingerprint())
            before_mode = fingerprint()
            git('update-index', '--chmod=+x', 'README.md'); git('commit', '-m', 'mode')
            self.assertNotEqual(before_mode, fingerprint())
