"""Run complete upstream level-registry methods against native Imre operations."""

import argparse
import ast
from contextlib import contextmanager
import io
from pathlib import Path
import subprocess
import sys
import tempfile
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
sys.path.insert(0, str(STDLIB))
from upstream import verify_upstream


class LevelRegistry:
    INFO = 20
    FATAL = 50
    NOTSET = 0

    def __init__(self, process):
        self.process = process

    def invoke(self, operation, *args):
        for arg in (operation, *args):
            line = str(arg)
            if "\n" in line or "\r" in line:
                raise ValueError("the test transport requires a single line")
            self.process.stdin.write(line + "\n")
        self.process.stdin.flush()
        value = self.process.stdout.readline()
        if not value:
            raise RuntimeError("native registry stopped before replying")
        return ast.literal_eval(value)

    def getLevelName(self, level):
        return self.invoke("name" if isinstance(level, str) else "number", level)

    def addLevelName(self, level, levelName):
        return self.invoke("add", level, levelName)


@contextmanager
def registry(binary):
    with subprocess.Popen([binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                          encoding="utf-8") as process:
        try:
            yield LevelRegistry(process)
        finally:
            if process.poll() is None:
                process.stdin.write("stop\n")
                process.stdin.close()
            if process.wait(timeout=10):
                raise RuntimeError(f"native registry exited {process.returncode}")


MAP_CASE = '''import logging

class Case:
    def assertEqual(self, left: dict[str, int], right: dict[str, int]) -> None:
        if len(left) != len(right):
            raise AssertionError("different mapping sizes")
        for key in left:
            if key not in right or left[key] != right[key]:
                raise AssertionError("different mapping contents")

    def assertIsNot(self, left: dict[str, int], right: dict[str, int]) -> None:
        key = ""
        while key in left or key in right:
            key += "."
        left[key] = 0
        aliased = key in right
        del left[key]
        if aliased:
            raise AssertionError("same dictionary")

'''

CUSTOM_CASE = '''import logging

class Case:
    root_logger: logging.Logger
    message_num: int

    def __init__(self) -> None:
        self.root_logger = logging.getLogger()
        self.message_num = 0

    def next_message(self) -> str:
        self.message_num += 1
        return str(self.message_num)

    def assert_log_lines(self, expected: list[tuple[str, str]]) -> None:
        for name, message in expected:
            print(name + "\\t" + message)

'''


def method_source(source, cls, name):
    node = next(n for n in ast.parse(source).body if isinstance(n, ast.ClassDef) and n.name == cls)
    method = next(n for n in node.body if isinstance(n, ast.FunctionDef) and n.name == name)
    assert not method.decorator_list
    return "".join(source.splitlines(keepends=True)[method.lineno - 1:method.end_lineno])


def run(args, test_logging, work):
    def build(source):
        binary = work / source.stem
        subprocess.run([args.imre.resolve(), "build", source, "-o", binary, "--no-cache",
                        "--opt-level", args.opt_level, "--runtime", args.runtime.resolve(),
                        "--clang", args.clang.resolve(),
                        *(["--sanitize"] if args.sanitize else [])], cwd=work, check=True, timeout=120)
        return binary

    changes = HERE / "level_changes.py"
    expected = subprocess.run([sys.executable, changes], capture_output=True, check=True, timeout=30)
    actual = subprocess.run([build(changes)], capture_output=True, check=True, timeout=30)
    assert (actual.stdout, actual.stderr) == (expected.stdout, expected.stderr)
    binary = build(HERE / "level_driver.py")
    for method in ("test_regression_22386", "test_issue27935", "test_regression_29220"):
        case = test_logging.BuiltinLevelsTest(method)
        with registry(binary) as native:
            # Run the original fixture around the method. Its own module state is
            # separate; the method and its registered cleanup use only the driver.
            original = getattr(case, method)
            def check():
                with patch.object(test_logging, "logging", native):
                    original()
            setattr(case, method, check)
            result = case.run()
            if not result.wasSuccessful() or result.skipped:
                raise AssertionError((case.id(), result.errors, result.failures, result.skipped))
        print("host-driven native operations", case.id())

    source = Path(test_logging.__file__).read_text()
    method = "test_get_level_names_mapping"
    program = work / "mapping.py"
    program.write_text(MAP_CASE + method_source(source, "ModuleLevelMiscTest", method)
                       + f"\nCase().{method}()\n")
    subprocess.run([build(program)], cwd=work, check=True, timeout=30)
    print("native body and operations", f"test.test_logging.ModuleLevelMiscTest.{method}")

    cls = "CustomLevelsAndFiltersTest"
    method = "test_logger_filter"
    program = work / "custom.py"
    levels = test_logging.my_logging_levels
    setup = "".join(f"logging.addLevelName({k}, {v!r})\n" for k, v in levels.items())
    constants = (f"VERBOSE = {test_logging.VERBOSE}\n"
                 + f"LEVEL_RANGE = {list(test_logging.LEVEL_RANGE)!r}\n")
    helper = method_source(source, cls, "log_at_all_levels")
    program.write_text(CUSTOM_CASE.replace("class Case:", constants + "\nclass Case:")
                       + helper + "\n" + method_source(source, cls, method) + "\n" + setup
                       + f"logging.basicConfig(format={test_logging.BaseTest.log_format!r})\n"
                       + f"Case().{method}()\n")
    run = subprocess.run([build(program)], cwd=work, capture_output=True, text=True,
                         check=True, timeout=30)
    expected = [tuple(row.split("\t")) for row in run.stdout.splitlines()]
    stream = io.StringIO()
    stream.write(run.stderr)
    test_logging.CustomLevelsAndFiltersTest().assert_log_lines(expected, stream)
    print("native body and operations, host output assertion", f"test.test_logging.{cls}.{method}")


def main():
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5) or not __debug__:
        raise SystemExit("run with CPython 3.14.5 and assertions enabled")
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang"):
        parser.add_argument("--" + name, required=True, type=Path)
    parser.add_argument("--opt-level", choices=("0", "1", "2", "3"), default="3")
    parser.add_argument("--sanitize", action="store_true")
    parser.add_argument("--cpython-source", type=Path)
    args = parser.parse_args()
    verify_upstream()
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    import test
    vendored = STDLIB / "upstream/Lib/test"
    test.__path__.insert(0, str(vendored))
    from test import test_logging
    assert Path(test_logging.__file__).resolve() == vendored / "test_logging.py"
    with tempfile.TemporaryDirectory() as tmp:
        run(args, test_logging, Path(tmp))


if __name__ == "__main__":
    main()
