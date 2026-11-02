"""Execute an interactive doctest in the boxed runtime."""
import sys
from io import StringIO


def ascii_output(text):
    return text.encode("ascii", "backslashreplace").decode("ascii")


def execute(test, debug):
    from doctest import _check_example, _exception_line, _optionflags, FAIL_FAST, SKIP

    failures = []
    saved_stdout = sys.stdout
    saved_displayhook = sys.displayhook
    stream = StringIO()
    try:
        sys.stdout = stream
        sys.displayhook = sys.__displayhook__
        for index, example in enumerate(test.examples):
            flags = _optionflags(example)
            if flags & SKIP:
                continue
            filename = f"<doctest {test.name}[{index}]>"
            error = ""
            try:
                exec(compile(example.source, filename, "single"), test.globs)
            except KeyboardInterrupt:
                raise
            except BaseException as exc:
                error = _exception_line(exc)
            got = stream.getvalue()
            stream.seek(0)
            stream.truncate(0)
            if got and not got.endswith("\n"):
                got += "\n"
            _check_example(test, example, got, error, failures, debug)
            if failures and flags & FAIL_FAST:
                break
    finally:
        sys.stdout = saved_stdout
        sys.displayhook = saved_displayhook
    if failures:
        raise AssertionError("\n".join(failures))
