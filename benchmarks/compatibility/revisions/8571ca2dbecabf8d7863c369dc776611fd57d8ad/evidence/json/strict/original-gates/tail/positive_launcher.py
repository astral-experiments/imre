"""Generated finite unittest launcher. Do not edit the generated copy."""
import json
import sys
import unittest


def emit(event):
    print("IMRE_UNITTEST " + json.dumps(event, ensure_ascii=True), file=sys.stderr, flush=True)


class OriginalResult(unittest.TestResult):
    def __init__(self):
        super().__init__()
        self.current = None
        self.outcome = None
        self.details = []

    def startTest(self, test):
        super().startTest(test)
        self.current = test
        self.outcome = "pass"
        self.details = []
        emit({"event": "start", "id": test.id()})

    def record(self, test, outcome, detail=None):
        if test is self.current:
            # An error takes precedence over a failure across multiple subtests.
            if self.outcome != "error" or outcome == "error":
                self.outcome = outcome
            if detail is not None:
                self.details.append(detail)
        else:
            emit({"event": "fixture", "id": test.id(), "outcome": outcome, "detail": detail})

    def addFailure(self, test, err):
        super().addFailure(test, err)
        self.record(test, "fail", str(err[1]))

    def addError(self, test, err):
        super().addError(test, err)
        self.record(test, "error", str(err[1]))

    def addSkip(self, test, reason):
        super().addSkip(test, reason)
        if test is self.current:
            self.record(test, "skip", reason)
        elif self.current is not None:
            emit({"event": "subtest_skip", "id": test.id(), "reason": reason})
        else:
            self.record(test, "skip", reason)

    def addSubTest(self, test, subtest, err):
        super().addSubTest(test, subtest, err)
        if err is not None:
            outcome = "fail" if issubclass(err[0], test.failureException) else "error"
            self.record(test, outcome, subtest.id() + ": " + str(err[1]))

    def addExpectedFailure(self, test, err):
        super().addExpectedFailure(test, err)
        self.record(test, "expected_failure", str(err[1]))

    def addUnexpectedSuccess(self, test):
        super().addUnexpectedSuccess(test)
        self.record(test, "unexpected_success")

    def stopTest(self, test):
        emit({"event": "result", "id": test.id(), "outcome": self.outcome, "details": self.details})
        super().stopTest(test)
        self.current = None


TAIL_IDS = ('test.test_json.test_recursion.TestPyRecursion.test_highly_nested_objects_encoding', 'test.test_json.test_recursion.TestPyRecursion.test_listrecursion', 'test.test_json.test_scanstring.TestCScanstring.test_bad_escapes', 'test.test_json.test_scanstring.TestCScanstring.test_overflow', 'test.test_json.test_scanstring.TestCScanstring.test_scanstring', 'test.test_json.test_scanstring.TestCScanstring.test_surrogates', 'test.test_json.test_scanstring.TestPyScanstring.test_bad_escapes', 'test.test_json.test_scanstring.TestPyScanstring.test_overflow', 'test.test_json.test_scanstring.TestPyScanstring.test_scanstring', 'test.test_json.test_scanstring.TestPyScanstring.test_surrogates', 'test.test_json.test_separators.TestCSeparators.test_illegal_separators', 'test.test_json.test_separators.TestCSeparators.test_separators', 'test.test_json.test_separators.TestPySeparators.test_illegal_separators', 'test.test_json.test_separators.TestPySeparators.test_separators', 'test.test_json.test_speedups.TestDecode.test_bad_bool_args', 'test.test_json.test_speedups.TestDecode.test_make_scanner', 'test.test_json.test_speedups.TestEncode.test_bad_bool_args', 'test.test_json.test_speedups.TestEncode.test_bad_markers_argument_to_encoder', 'test.test_json.test_speedups.TestEncode.test_bad_str_encoder', 'test.test_json.test_speedups.TestEncode.test_current_indent_level', 'test.test_json.test_speedups.TestEncode.test_make_encoder', 'test.test_json.test_speedups.TestEncode.test_unsortable_keys', 'test.test_json.test_speedups.TestSpeedups.test_encode_basestring_ascii', 'test.test_json.test_speedups.TestSpeedups.test_scanstring', 'test.test_json.test_tool.TestMain.test_broken_pipe_error', 'test.test_json.test_tool.TestMain.test_colors', 'test.test_json.test_tool.TestMain.test_compact', 'test.test_json.test_tool.TestMain.test_ensure_ascii_default', 'test.test_json.test_tool.TestMain.test_help_flag', 'test.test_json.test_tool.TestMain.test_indent', 'test.test_json.test_tool.TestMain.test_infile_outfile', 'test.test_json.test_tool.TestMain.test_infile_stdout', 'test.test_json.test_tool.TestMain.test_jsonlines', 'test.test_json.test_tool.TestMain.test_jsonlines_from_file', 'test.test_json.test_tool.TestMain.test_no_ensure_ascii_flag', 'test.test_json.test_tool.TestMain.test_no_indent', 'test.test_json.test_tool.TestMain.test_non_ascii_infile', 'test.test_json.test_tool.TestMain.test_sort_keys_flag', 'test.test_json.test_tool.TestMain.test_stdin_stdout', 'test.test_json.test_tool.TestMain.test_tab', 'test.test_json.test_tool.TestMain.test_writing_in_place', 'test.test_json.test_tool.TestTool.test_broken_pipe_error', 'test.test_json.test_tool.TestTool.test_colors', 'test.test_json.test_tool.TestTool.test_compact', 'test.test_json.test_tool.TestTool.test_ensure_ascii_default', 'test.test_json.test_tool.TestTool.test_help_flag', 'test.test_json.test_tool.TestTool.test_indent', 'test.test_json.test_tool.TestTool.test_infile_outfile', 'test.test_json.test_tool.TestTool.test_infile_stdout', 'test.test_json.test_tool.TestTool.test_jsonlines', 'test.test_json.test_tool.TestTool.test_jsonlines_from_file', 'test.test_json.test_tool.TestTool.test_no_ensure_ascii_flag', 'test.test_json.test_tool.TestTool.test_no_indent', 'test.test_json.test_tool.TestTool.test_non_ascii_infile', 'test.test_json.test_tool.TestTool.test_sort_keys_flag', 'test.test_json.test_tool.TestTool.test_stdin_stdout', 'test.test_json.test_tool.TestTool.test_tab', 'test.test_json.test_tool.TestTool.test_writing_in_place', 'test.test_json.test_unicode.TestCUnicode.test_ascii_non_printable_decode', 'test.test_json.test_unicode.TestCUnicode.test_ascii_non_printable_encode', 'test.test_json.test_unicode.TestCUnicode.test_big_unicode_decode', 'test.test_json.test_unicode.TestCUnicode.test_big_unicode_encode', 'test.test_json.test_unicode.TestCUnicode.test_bytes_decode', 'test.test_json.test_unicode.TestCUnicode.test_bytes_encode', 'test.test_json.test_unicode.TestCUnicode.test_encoding3', 'test.test_json.test_unicode.TestCUnicode.test_encoding4', 'test.test_json.test_unicode.TestCUnicode.test_encoding5', 'test.test_json.test_unicode.TestCUnicode.test_encoding6', 'test.test_json.test_unicode.TestCUnicode.test_encoding7', 'test.test_json.test_unicode.TestCUnicode.test_escaped_decode', 'test.test_json.test_unicode.TestCUnicode.test_object_pairs_hook_with_unicode', 'test.test_json.test_unicode.TestCUnicode.test_single_surrogate_decode', 'test.test_json.test_unicode.TestCUnicode.test_single_surrogate_encode', 'test.test_json.test_unicode.TestCUnicode.test_unicode_decode', 'test.test_json.test_unicode.TestCUnicode.test_unicode_preservation', 'test.test_json.test_unicode.TestPyUnicode.test_ascii_non_printable_decode', 'test.test_json.test_unicode.TestPyUnicode.test_ascii_non_printable_encode', 'test.test_json.test_unicode.TestPyUnicode.test_big_unicode_decode', 'test.test_json.test_unicode.TestPyUnicode.test_big_unicode_encode', 'test.test_json.test_unicode.TestPyUnicode.test_bytes_decode', 'test.test_json.test_unicode.TestPyUnicode.test_bytes_encode', 'test.test_json.test_unicode.TestPyUnicode.test_encoding3', 'test.test_json.test_unicode.TestPyUnicode.test_encoding4', 'test.test_json.test_unicode.TestPyUnicode.test_encoding5', 'test.test_json.test_unicode.TestPyUnicode.test_encoding6', 'test.test_json.test_unicode.TestPyUnicode.test_encoding7', 'test.test_json.test_unicode.TestPyUnicode.test_escaped_decode', 'test.test_json.test_unicode.TestPyUnicode.test_object_pairs_hook_with_unicode', 'test.test_json.test_unicode.TestPyUnicode.test_single_surrogate_decode', 'test.test_json.test_unicode.TestPyUnicode.test_single_surrogate_encode', 'test.test_json.test_unicode.TestPyUnicode.test_unicode_decode', 'test.test_json.test_unicode.TestPyUnicode.test_unicode_preservation')

def select_originals(source, target, wanted):
    for original in source:
        if isinstance(original, unittest.TestSuite):
            select_originals(original, target, wanted)
        elif original.id() in wanted:
            target.addTest(original)


def collect():
    loader = unittest.TestLoader()
    suite = unittest.TestSuite()
    import test.test_json.test_recursion as _module_0
    suite.addTests(loader.loadTestsFromModule(_module_0))
    import test.test_json.test_scanstring as _module_1
    suite.addTests(loader.loadTestsFromModule(_module_1))
    import test.test_json.test_separators as _module_2
    suite.addTests(loader.loadTestsFromModule(_module_2))
    import test.test_json.test_speedups as _module_3
    suite.addTests(loader.loadTestsFromModule(_module_3))
    import test.test_json.test_tool as _module_4
    suite.addTests(loader.loadTestsFromModule(_module_4))
    import test.test_json.test_unicode as _module_5
    suite.addTests(loader.loadTestsFromModule(_module_5))
    wanted = TAIL_IDS[int(sys.argv[1]):int(sys.argv[2])]
    selected = unittest.TestSuite()
    select_originals(suite, selected, wanted)
    return selected


def inventory(suite):
    for test in suite:
        if isinstance(test, unittest.TestSuite):
            inventory(test)
        else:
            emit({"event": "inventory", "id": test.id()})


def main():
    suite = collect()
    emit({"event": "collection", "count": suite.countTestCases()})
    inventory(suite)
    result = OriginalResult()
    suite.run(result)
    emit({"event": "summary", "run": result.testsRun, "successful": result.wasSuccessful()})
    return 0 if result.wasSuccessful() else 1


if __name__ == "__main__":
    raise SystemExit(main())
