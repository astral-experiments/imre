"""Run the pinned ownership, disk, and terminal fixtures with native filesystem operations."""
import argparse
import ast
from contextlib import chdir
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
sys.path.insert(0, str(STDLIB))
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, run_in_fixtures


def spelling(value, slot):
    """Transport a test argument without performing the operation."""
    arg = f"sys.argv[{slot}]"
    if value is None:
        return "None", ""
    if isinstance(value, bytes):
        return repr(value), ""
    if isinstance(value, bool):
        return f"{arg} == 'True'", str(value)
    if isinstance(value, int):
        return f"int({arg})", str(value)
    if isinstance(value, float):
        return f"float({arg})", str(value)
    if isinstance(value, str):
        return arg, value
    if isinstance(value, Path):
        return f"Path({arg})", os.fspath(value)
    raise TypeError(type(value))


ERROR_HANDLERS = """except OSError as error:
    print(repr(error))
    print(repr(str(error)))
    print(repr(error.args))
    print(repr(error.errno))
    print(repr(error.strerror))
    print(repr(error.filename))
    print(repr(error.filename2))
except (ValueError, LookupError, TypeError, OverflowError) as error:
    print(repr(error))
    print(repr(str(error)))"""


def raise_native_error(output):
    representation, message, *attributes = output.splitlines()
    expr = ast.parse(representation, mode="eval").body
    classes = {c.__name__: c for c in (ValueError, LookupError, TypeError, OverflowError,
               OSError, FileNotFoundError, PermissionError, NotADirectoryError)}
    kind = classes[expr.func.id]
    args = tuple(ast.literal_eval(arg) for arg in expr.args)
    if issubclass(kind, OSError):
        native_args, errno, reason, first, second = map(ast.literal_eval, attributes)
        assert native_args == args
        operands = () if first is None else (first,) if second is None else (first, None, second)
        error = kind(*args, *operands)
        assert (error.args, error.errno, error.strerror, error.filename, error.filename2) == (
            native_args, errno, reason, first, second)
    else:
        assert not attributes
        error = kind(*args)
    assert repr(error) == representation and str(error) == ast.literal_eval(message)
    raise error


class Ownership:
    def __init__(self, tools, folder):
        self.tools = tools
        self.folder = folder
        self.binaries = {}
        self.calls = 0

    def __call__(self, path, user=None, group=None, *, dir_fd=None, follow_symlinks=True):
        self.calls += 1
        expressions, values = zip(*(spelling(v, i) for i, v in enumerate(
            (path, user, group, dir_fd, follow_symlinks), 1)))
        if expressions not in self.binaries:
            sub = self.folder / f"chown-{len(self.binaries)}"
            sub.mkdir()
            p, u, g, d, f = expressions
            source = f"""import shutil
import sys
from pathlib import Path
try:
    shutil.chown({p}, {u}, {g}, dir_fd={d}, follow_symlinks={f})
    print('ok')
{ERROR_HANDLERS}
"""
            self.binaries[expressions] = self.tools.build(source.encode(), sub)
        fds = []
        for fd in (path, dir_fd):
            if isinstance(fd, int) and 0 <= fd <= 2**31 - 1:
                try:
                    os.fstat(fd)
                except OSError:
                    continue
                fds.append(fd)
        result = subprocess.run([self.binaries[expressions], *values], pass_fds=tuple(set(fds)),
                                capture_output=True, text=True, timeout=30, check=True)
        assert not result.stderr, result.stderr
        if result.stdout == "ok\n":
            return None
        raise_native_error(result.stdout)


def run_level(tools, main, folder):
    suite = load_suite('test.test_shutil')
    ownership = Ownership(tools, folder)
    def execute(case, original):
        name = case._testMethodName
        if name == 'test_chown':
            with patch.object(main.shutil, 'chown', ownership):
                original()
            assert ownership.calls
            return {"status": "host_driven_native", "operations": {"chown": ownership.calls},
                    "fixture": "original OS account lookup, descriptors, stat assertions and cleanups in CPython"}
        return {"status": "blocked", "reason": "native named tuple result and original fixtures pending"}
    selected = {case.id(): execute for case in cases(suite)
                if isinstance(case, (main.TestMisc, main.TestGetTerminalSize))}
    good, failures, skips = run_in_fixtures(suite, selected)
    rows = {n: v[0] for n, v in good.items() if v}
    rows.update({n: {"status": "failed", "reason": reason} for n, reason in failures})
    rows.update({n: {"status": "reference_skip", "reason": reason} for n, reason in skips})
    assert rows.keys() == selected.keys() and len(rows) == 7
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('imre', 'runtime', 'clang', 'result'):
        parser.add_argument('--' + name, type=lambda p: Path(p).resolve(), required=True)
    args = parser.parse_args()
    if sys.implementation.name != 'cpython' or sys.version_info[:3] != (3, 14, 5) or sys.platform != 'linux':
        parser.error("requires the test-enabled CPython 3.14.5 Linux interpreter")
    manifest = json.loads((HERE / 'UPSTREAM.json').read_text())
    verify_files(STDLIB / 'upstream', manifest['files'])
    verify_interpreter_tests(manifest)
    import test
    source = STDLIB / 'upstream/Lib/test'
    test.__path__.insert(0, str(source))
    levels = {}
    with tempfile.TemporaryDirectory() as tmp, chdir(tmp):
        from test import test_shutil
        assert Path(test_shutil.__file__).resolve() == source / 'test_shutil.py'
        for level in (0, 3):
            folder = Path(tmp) / str(level)
            folder.mkdir()
            levels[level] = run_level(Toolchain(args.imre, args.runtime, args.clang, level),
                                      test_shutil, folder)
    args.result.write_text(json.dumps({"cpython_commit": manifest["commit"],
                                      "optimization_levels": levels}, indent=2) + "\n")
    return int(any(v['status'] == 'failed' for rows in levels.values() for v in rows.values()))


if __name__ == '__main__':
    sys.exit(main())
