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
from suite_native import Toolchain, native_method, run_in_fixtures, testcase_fixture
from suite_evidence import roles


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


SIZE_DRIVER = """import shutil
import sys
with open(sys.argv[1], 'w', encoding='utf-8') as output:
    if sys.argv[2] == 'disk':
        usage = shutil.disk_usage(sys.argv[3])
        output.write(str(usage.total) + ' ' + str(usage.used) + ' ' + str(usage.free))
    else:
        size = shutil.get_terminal_size((int(sys.argv[3]), int(sys.argv[4])))
        output.write(str(size.columns) + ' ' + str(size.lines))
"""


def size_driver(tools, folder):
    work = folder / "size"
    work.mkdir()
    binary = tools.build(SIZE_DRIVER.encode(), work)
    def invoke(*argv, stream=None, closed=False):
        with tempfile.NamedTemporaryFile(dir=work) as output:
            # A swapped __stdout__ is fixture state. Give the child the actual
            # descriptor; a missing __stdout__ maps to a closed fd 1.
            result = subprocess.run([binary, output.name, *argv], stdin=subprocess.DEVNULL,
                                    stdout=stream, stderr=subprocess.PIPE,
                                    preexec_fn=(lambda: os.close(1)) if closed else None,
                                    timeout=30, check=True)
            assert not result.stderr, result.stderr
            return tuple(map(int, output.read().split()))
    return invoke


def executors(tools, main, folder, *, with_roles=False):
    suite = load_suite('test.test_shutil')
    ownership = Ownership(tools, folder)
    size = size_driver(tools, folder)
    def execute(case, original):
        name = case._testMethodName
        if name == 'test_chown':
            with patch.object(main.shutil, 'chown', ownership):
                original()
            assert ownership.calls
            return {"status": "host_driven_native", "operations": {"chown": ownership.calls},
                    "fixture": "original OS account lookup, descriptors, stat assertions and cleanups in CPython"}
        if name == 'test_does_not_crash':
            work = folder / name
            work.mkdir()
            fixture = "import shutil\n" + testcase_fixture()
            run = native_method(tools, work, Path(main.__file__), 'TestGetTerminalSize',
                                case.id(), fixture, operation=('imre', 'shutil.get_terminal_size'))
            return {"status": "native_body_host_fixture", **run(case, original)}
        calls = 0
        if name == 'test_disk_usage':
            def disk(path):
                nonlocal calls
                calls += 1
                return main.shutil._ntuple_diskusage(*size('disk', path))
            with patch.object(main.shutil, 'disk_usage', disk):
                original()
            assert calls == 2
            return {"status": "host_driven_native", "operations": {"disk_usage": calls},
                    "fixture": "both native statvfs results; original dynamic field/type assertions in CPython"}
        def terminal(fallback=(80, 24)):
            nonlocal calls
            calls += 1
            stream = sys.__stdout__
            dimensions = size('terminal', *map(str, fallback), stream=stream, closed=stream is None)
            return os.terminal_size(dimensions)
        with patch.object(main.shutil, 'get_terminal_size', terminal):
            original()
        assert calls > 0
        return {"status": "mixed" if name == 'test_fallback' else "host_driven_native",
                "operations": {"get_terminal_size": calls},
                "fixture": ("original sys.__stdout__ swaps translated to a closed stdout and the test's /dev/null descriptor"
                            if name == 'test_fallback' else "original process environment forwarded; assertions in CPython")}
    def observe(case, original):
        result = execute(case, original)
        if not with_roles:
            return result
        if "observation" in result:
            return result["observation"]
        (operation, calls), = result["operations"].items()
        return {"roles": roles(body="cpython", assertions="cpython",
                               operation="mixed" if result["status"] == "mixed" else "imre",
                               fixture="cpython"),
                "evidence": {"operation": "shutil." + operation, "native_calls": calls,
                             "fixture": result["fixture"], "opt_level": tools.level}}

    return {case.id(): observe for case in cases(suite)
            if isinstance(case, (main.TestMisc, main.TestGetTerminalSize))}


def run_level(tools, main, folder):
    suite = load_suite("test.test_shutil")
    selected = executors(tools, main, folder)
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
    parser.add_argument("--controls", action="store_true", help="compare extra real fd, terminal and exception cases with CPython")
    parser.add_argument("--cpython-source", type=Path)
    args = parser.parse_args()
    if sys.implementation.name != 'cpython' or sys.version_info[:3] != (3, 14, 5) or sys.platform != 'linux':
        parser.error("requires the test-enabled CPython 3.14.5 Linux interpreter")
    manifest = json.loads((HERE / 'UPSTREAM.json').read_text())
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
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
            tools = Toolchain(args.imre, args.runtime, args.clang, level)
            levels[level] = run_level(tools, test_shutil, folder)
            if args.controls:
                from ownership_controls import compare as check_ownership
                from capacity_controls import compare as check_capacity
                ownership = folder / "ownership-controls"
                ownership.mkdir()
                print(f"-O{level}: {check_ownership(tools, ownership)} ownership and "
                      f"{check_capacity(tools, folder)} capacity controls matched CPython", flush=True)
    args.result.write_text(json.dumps({"cpython_commit": manifest["commit"],
                                      "optimization_levels": levels}, indent=2) + "\n")
    return int(any(v['status'] == 'failed' for rows in levels.values() for v in rows.values()))


if __name__ == '__main__':
    sys.exit(main())
