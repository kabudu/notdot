import unittest
from build_windows_wake_helper import optimized_build


class WindowsHelperOptimizationTests(unittest.TestCase):
    def cache(self, flags='/O2 /Ob2 /DNDEBUG', generator='Ninja'):
        return f'CMAKE_GENERATOR:INTERNAL={generator}\nCMAKE_BUILD_TYPE:STRING=Release\nCMAKE_CXX_FLAGS:STRING=/clang:-mno-sahf\nCMAKE_CXX_FLAGS_RELEASE:STRING={flags}\nCMAKE_CXX_FLAGS_DEBUG:STRING=/Od /Zi\n'

    def test_effective_release_optimization_accepts_debug_profile_metadata(self):
        self.assertEqual(optimized_build(self.cache())['build_type'], 'Release')

    def test_implicit_visual_studio_configuration_is_rejected(self):
        with self.assertRaisesRegex(ValueError, 'Ninja'):
            optimized_build(self.cache(generator='Visual Studio 17 2022'))

    def test_release_flags_without_optimization_are_rejected(self):
        with self.assertRaisesRegex(ValueError, 'optimization'):
            optimized_build(self.cache('/MT /utf-8'))

    def test_disabling_optimization_after_o2_is_rejected(self):
        with self.assertRaisesRegex(ValueError, 'optimization'):
            optimized_build(self.cache('/O2 /Od'))

    def test_cross_compiler_release_flags_are_accepted(self):
        self.assertEqual(optimized_build(self.cache('-O3 -DNDEBUG'))['generator'], 'Ninja')
