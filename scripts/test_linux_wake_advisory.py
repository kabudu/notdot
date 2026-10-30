import unittest
from check_linux_wake_advisory import classify, MARKER, TEST


def transcript(message=None):
    status = 'ok' if message is None else 'FAILED'
    panic = '' if message is None else f"thread '{TEST}' (123) panicked at src-tauri/src/wake.rs:1880:9:\n{message}\n"
    passed, failed = (1, 0) if message is None else (0, 1)
    return f'running 1 test\n{panic}test {TEST} ... {status}\ntest result: {status}. {passed} passed; {failed} failed; 0 ignored; 0 measured; 68 filtered out; finished in 1.20s\n'


class LinuxWakeAdvisoryTests(unittest.TestCase):
    def test_actual_pass_and_only_the_completed_accuracy_assertion_are_accepted(self):
        self.assertEqual(classify(transcript(), 0), 'passed')
        self.assertEqual(classify(transcript(MARKER), 101), 'accuracy-miss')

    def test_decode_error_or_stuck_worker_is_blocking(self):
        for message in ['fixture decode failed: Local wake recognition exceeded its time limit', 'current fixture did not settle']:
            with self.subTest(message=message), self.assertRaises(ValueError):
                classify(transcript(message), 101)

    def test_marker_in_compiler_output_does_not_authorise_a_failure(self):
        with self.assertRaises(ValueError):
            classify(f'warning: assert!(detected, "{MARKER}")\n' + transcript('different panic'), 101)

    def test_native_abort_even_after_a_failed_footer_is_blocking(self):
        for diagnostic in ['signal: 6, SIGABRT: process abort signal', 'free(): invalid pointer', 'Segmentation fault (core dumped)', 'Aborted', 'Segmentation fault']:
            with self.subTest(diagnostic=diagnostic), self.assertRaises(ValueError):
                classify(transcript(MARKER) + diagnostic, 101)

    def test_no_test_compiler_failure_or_missing_footer_is_blocking(self):
        for text, code in [('error: could not compile notdot', 101), ('running 0 tests\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 69 filtered out; finished in 0.00s\n', 0), (transcript(MARKER).split('test result:')[0], 101)]:
            with self.subTest(text=text), self.assertRaises(ValueError):
                classify(text, code)

    def test_wrong_case_duplicate_footer_or_wrong_exit_code_is_blocking(self):
        for text, code in [(transcript().replace(TEST, 'different::test'), 0), (transcript(MARKER) * 2, 101), (transcript(MARKER), 134), (transcript(), 101)]:
            with self.subTest(code=code), self.assertRaises(ValueError):
                classify(text, code)

    def test_colour_does_not_hide_the_test_result(self):
        text = transcript(MARKER).replace('FAILED', '\x1b[31mFAILED\x1b[0m')
        self.assertEqual(classify(text, 101), 'accuracy-miss')
