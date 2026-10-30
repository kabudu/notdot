import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock

from repair_sherpa_cache import repair


class RepairSherpaCacheTests(unittest.TestCase):
    def test_pruned_directories_and_cached_paths_are_discarded(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'sherpa-onnx-prebuilt/archive/lib').mkdir(parents=True)
            unrelated = root / 'debug/other-package'
            unrelated.mkdir(parents=True)
            run = Mock()
            repair(root, ['x86_64-pc-windows-msvc'], run)
            self.assertFalse((root / 'sherpa-onnx-prebuilt').exists())
            self.assertTrue(unrelated.exists())
            self.assertEqual(run.call_count, 2)
            self.assertEqual(run.call_args_list[0].args[0], ['cargo', 'clean', '-p', 'sherpa-onnx-sys', '--target-dir', str(root)])
            self.assertEqual(run.call_args_list[1].args[0][-2:], ['--target', 'x86_64-pc-windows-msvc'])
            self.assertTrue(all(call.kwargs['check'] for call in run.call_args_list))

    def test_universal_invalidates_both_actual_architectures(self):
        with tempfile.TemporaryDirectory() as tmp:
            run = Mock()
            repair(Path(tmp), ['aarch64-apple-darwin', 'x86_64-apple-darwin'], run)
            self.assertEqual(run.call_count, 3)
            self.assertEqual([call.args[0][-1] for call in run.call_args_list[1:]], ['aarch64-apple-darwin', 'x86_64-apple-darwin'])

    def test_symlink_cache_refused_without_deleting_external_files(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            external = root / 'external'
            external.mkdir()
            (root / 'sherpa-onnx-prebuilt').symlink_to(external, target_is_directory=True)
            run = Mock()
            with self.assertRaises(ValueError):
                repair(root, [], run)
            self.assertTrue(external.is_dir())
            run.assert_not_called()

    def test_clean_failure_blocks_release(self):
        with tempfile.TemporaryDirectory() as tmp:
            run = Mock(side_effect=RuntimeError('failed'))
            with self.assertRaises(RuntimeError):
                repair(Path(tmp), ['x86_64-pc-windows-msvc'], run)
            self.assertEqual(run.call_count, 1)


if __name__ == '__main__':
    unittest.main()
