import io
import sys
import unittest


def current():
    console_out, console_err = sys.stdout, sys.stderr
    original_out, original_err = io.StringIO(), io.StringIO()
    try:
        sys.stdout, sys.stderr = original_out, original_err
        result = unittest.TestResult()
        result.buffer = True
        first, second = unittest.TestCase(), unittest.TestCase()

        result.startTest(first)
        out, err = sys.stdout, sys.stderr
        assert out is not original_out and err is not original_err
        out.write("before")
        err.write("problem\n")
        result.addFailure(first, (AssertionError, AssertionError("failed"), None))
        owner, message = result.failures[0]
        assert owner is first
        assert message == "AssertionError: failed\n\nStdout:\nbefore\n\nStderr:\nproblem\n"

        # stopTest mirrors the streams as they stand after the failure callback.
        out.write(" and after")
        result.stopTest(first)
        assert sys.stdout is original_out and sys.stderr is original_err
        assert original_out.getvalue() == "\nStdout:\nbefore and after\n"
        assert original_err.getvalue() == "\nStderr:\nproblem\n"

        result.startTest(second)
        assert sys.stdout is out and sys.stderr is err
        assert out.getvalue() == err.getvalue() == ""
        print("successful output")
        print("successful error output", file=sys.stderr)
        result.addSuccess(second)
        result.stopTest(second)
        assert sys.stdout is original_out and sys.stderr is original_err
        assert original_out.getvalue() == "\nStdout:\nbefore and after\n"
        assert original_err.getvalue() == "\nStderr:\nproblem\n"

        class Failing(unittest.TestCase):
            @classmethod
            def setUpClass(cls):
                print("setup output")
                raise RuntimeError("setup failure")

            def runTest(self):
                raise AssertionError("test body ran")

        report = io.StringIO()
        suite = unittest.TestSuite([Failing()])
        runner_result = unittest.TextTestRunner(stream=report, buffer=True).run(suite)
        assert runner_result.testsRun == 0 and len(runner_result.errors) == 1
        owner, message = runner_result.errors[0]
        assert str(owner).startswith("setUpClass (")
        captured = "\nStdout:\nsetup output\n"
        assert message.endswith("RuntimeError: setup failure\n" + captured)
        assert captured in report.getvalue()
        assert original_out.getvalue() == "\nStdout:\nbefore and after\n" + captured
        assert sys.stdout is original_out and sys.stderr is original_err
    finally:
        sys.stdout, sys.stderr = console_out, console_err
