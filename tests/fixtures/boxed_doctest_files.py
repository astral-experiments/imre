import doctest
import os
import tempfile
import unittest


class RecordingChecker(doctest.OutputChecker):
    def __init__(self):
        self.wants = []

    def check_output(self, want, got, optionflags):
        self.wants.append(want)
        return super().check_output(want, got, optionflags)


def current():
    file_suite = doctest.DocFileSuite
    with tempfile.TemporaryDirectory() as root:
        good = os.path.join(root, "numbers.good.txt")
        wrong = os.path.join(root, "wrong.txt")
        with open(good, "w", encoding="utf-8") as file:
            file.write(
                ">>> __file__.endswith('numbers.good.txt')\nTrue\n"
                ">>> seed += 3\n>>> seed\n45\n"
                ">>> raise ValueError('example')\nTraceback (most recent call last):\n"
                "    ...\nValueError: example\n"
            )
        with open(wrong, "w", encoding="utf-8") as file:
            file.write(">>> seed\n41\n")

        globs = {"seed": 42}
        suite = file_suite(good, wrong, module_relative=False, globs=globs)
        assert [case.id() for case in suite] == ["numbers_good_txt", "wrong_txt"]
        for _ in range(2):
            result = unittest.TestResult()
            suite.run(result)
            assert result.testsRun == 2 and not result.errors and not result.skipped
            assert len(result.failures) == 1, result.failures
            case, message = result.failures[0]
            assert case.id() == "wrong_txt"
            expected, actual = message.split("Expected:", 1)[1].split("Got:", 1)
            assert expected.strip() == "41" and actual.strip().splitlines()[0] == "42"
        assert globs == {"seed": 42}

        with open(wrong, "w", encoding="utf-8") as file:
            file.write(">>> 2 + 2\n5\n>>> 5 + 5\n11\n>>> events.append('later')\n")
        relative = os.path.relpath(wrong, os.path.dirname(__file__))
        for flags, reports, wants, phases in (
            (0, 2, ["5\n", "11\n", ""], ["setup", "later", "teardown"]),
            (doctest.FAIL_FAST, 1, ["5\n"], ["setup", "teardown"]),
            (doctest.REPORT_ONLY_FIRST_FAILURE, 1, ["5\n", "11\n", ""], ["setup", "later", "teardown"]),
        ):
            events = []
            checker = RecordingChecker()

            def setup(test):
                events.append("setup")
                test.globs["events"] = events

            def teardown(test):
                test.globs["events"].append("teardown")

            suite = file_suite(relative, optionflags=flags, checker=checker, setUp=setup, tearDown=teardown)
            result = unittest.TestResult()
            suite.run(result)
            assert result.testsRun == 1 and not result.errors and not result.skipped
            assert len(result.failures) == 1
            message = result.failures[0][1]
            assert message.count("Failed example:") == reports
            assert "2 + 2" in message and ("5 + 5" in message) == (reports == 2)
            assert checker.wants == wants and events == phases

        events = []
        (case,) = file_suite(relative, setUp=setup, tearDown=teardown)
        try:
            case.debug()
        except doctest.DocTestFailure:
            pass
        else:
            raise AssertionError("debug did not stop at the failing prompt")
        assert events == ["setup"] and case._dt_test.globs["events"] is events

        with open(wrong, "w", encoding="utf-8") as file:
            file.write(">>> raise ValueError('actual')\nTraceback (most recent call last):\n"
                       "...\nValueError: expected\n")
        comparisons = []

        class RejectingChecker(doctest.OutputChecker):
            def check_output(self, want, got, optionflags):
                comparisons.append((want, got))
                return False

        result = unittest.TestResult()
        file_suite(relative, optionflags=doctest.IGNORE_EXCEPTION_DETAIL, checker=RejectingChecker()).run(result)
        assert len(result.failures) == 1 and not result.errors
        assert comparisons == [("ValueError: expected\n", "ValueError: actual\n"),
                               ("ValueError", "ValueError")]
    print("boxed doctest files")


if __name__ == "__main__":
    current()
