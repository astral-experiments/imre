"""Execute an interactive doctest in the boxed runtime."""
import sys
from io import StringIO


def exception_name(exc):
    cls = type(exc)
    name = cls.__qualname__
    module = cls.__module__
    if module not in ("__main__", "builtins"):
        if not isinstance(module, str):
            module = "<unknown>"
        name = module + "." + name
    return name


def execute(test, debug, optionflags=0, checker=None):
    from _imre_importlib import compile_single
    from doctest import (_check_output, _exception_line, _optionflags, OutputChecker,
                         FAIL_FAST, REPORT_ONLY_FIRST_FAILURE, SKIP)
    from unittest import SkipTest

    failures = []
    failure_count = 0
    attempted = 0
    flags = optionflags
    if checker is None:
        checker = OutputChecker()
    saved_stdout = sys.stdout
    saved_displayhook = sys.displayhook
    stream = StringIO()
    try:
        sys.stdout = stream
        sys.displayhook = sys.__displayhook__
        for index, example in enumerate(test.examples):
            # DocTestRunner decides whether to report before applying this prompt's directives.
            quiet = failure_count > 0 and bool(flags & REPORT_ONLY_FIRST_FAILURE)
            flags = _optionflags(example, optionflags)
            if flags & SKIP:
                continue
            attempted += 1
            filename = f"<doctest {test.name}[{index}]>"
            error = ""
            exc_info = None
            try:
                exec(compile_single(compile, example.source, filename, test), test.globs)
            except KeyboardInterrupt:
                raise
            except BaseException as exc:
                if debug:
                    tb = exc.__traceback__
                    exc_info = (type(exc), exc, tb.tb_next if tb is not None else None)
                # Formatting can write to stdout. Capture it with the raising prompt.
                error = _exception_line(exc)
            got = stream.getvalue()
            stream.seek(0)
            stream.truncate(0)
            if got and not got.endswith("\n"):
                got += "\n"
            if not _check_output(test, example, got, error, failures, debug, checker, flags, quiet, exc_info):
                failure_count += 1
            if failure_count and flags & FAIL_FAST:
                break
    finally:
        sys.stdout = saved_stdout
        sys.displayhook = saved_displayhook
    if failures:
        raise AssertionError("\n".join(failures))
    if not attempted and not debug:
        raise SkipTest("all examples were skipped")
