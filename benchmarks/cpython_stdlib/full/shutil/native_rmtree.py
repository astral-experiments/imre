"""Run CPython's original recursive removal routines against native directory descriptors."""
import argparse
import ast
import builtins
from contextlib import chdir
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from copyfile import STDLIB, build
import rmtree_handlers
from suite_evidence import roles
from suite_inventory import load_suite, verify_files, verify_interpreter_tests
from suite_native import run_in_fixtures

CASES = {
    "test_rmtree_errors": 8, "test_rmtree_dont_delete_file": 1, "test_rmtree_with_dir_fd": 1,
    "test_rmtree_works_on_bytes": 1, "test_rmtree_works_on_symlinks": 1,
    "test_rmtree_on_symlink": 3, "test_rmtree_on_named_pipe": 2,
    "test_rmtree_above_recursion_limit": 1,
}
CALLBACKS = {
    "test_both_onerror_and_onexc": 1,
    "test_on_error": 1,
    "test_on_exc": 1,
    "test_rmtree_deleted_race_condition": 1,
    "test_rmtree_errors_onerror": 1,
    "test_rmtree_errors_onexc": 1,
    "test_rmtree_fails_on_symlink_onerror": 2,
    "test_rmtree_fails_on_symlink_onexc": 2,
}
BLOCKERS = {
    "test_rmtree_does_not_choke_on_failing_lstat": "replaces os.lstat during traversal",
    "test_rmtree_fails_on_close": "patches os.close and requires the operation to observe the injected close error",
    "test_rmtree_uses_safe_fd_version_if_available": "replaces os.open with a custom exception-raising function",
}
ERRORS = ("FileNotFoundError", "PermissionError", "NotADirectoryError", "TypeError",
          "OverflowError", "ValueError", "OSError")


def driver():
    source = '''import shutil
import sys

def raw(filename: str) -> bytes:
    with open(filename, "rb") as file:
        return file.read()

if sys.argv[1] == "safe":
    print(shutil.rmtree.avoids_symlink_attacks)
else:
    try:
'''
    for key, path, fd in (("text", "sys.argv[2]", ""), ("bytes", "raw(sys.argv[2])", ""),
                          ("none", "None", ""), ("fd", "sys.argv[2]", ", dir_fd=int(sys.argv[4])"),
                          ("bad-fd", "sys.argv[2]", ", dir_fd=sys.argv[4]")):
        source += (f'        if sys.argv[1] == "{key}":\n'
                   f'            shutil.rmtree({path}, ignore_errors=sys.argv[3] == "1"{fd})\n')
    source += '        print("ok")\n'
    for cls in ERRORS:
        source += f'    except {cls} as exc:\n        print("{cls}")\n        print(repr(str(exc)))\n        print(repr(exc))\n'
        if issubclass(getattr(builtins, cls), OSError):
            source += '        print(repr(exc.errno))\n        print(repr(exc.strerror))\n        print(repr(exc.filename))\n        print(repr(exc.filename2))\n'
    return source.encode()


def exception(lines):
    exc = rmtree_handlers.native_error(lines[2], ast.literal_eval(lines[1]),
                                       [ast.literal_eval(val) for val in lines[3:]])
    assert type(exc).__name__ == lines[0]
    return exc


def executors(args, suite, folder, level, *, with_roles=False):
    exe = build(args, folder, "remove", driver(), level)
    callback = rmtree_handlers.build_driver(args, folder, driver(), level) if sys.platform == "linux" else None
    events = []
    def call(*args, pass_fds=()):
        result = subprocess.run([exe, *args], pass_fds=pass_fds, check=True, capture_output=True,
                                text=True, timeout=30)
        if result.stderr:
            raise AssertionError(result.stderr)
        return result.stdout.splitlines()
    def remove(path, ignore_errors=False, onerror=None, *, onexc=None, dir_fd=None):
        nonlocal calls
        mode = "text"
        fds = ()
        if isinstance(path, bytes):
            mode = "bytes"
            raw = folder / "argument.bin"
            raw.write_bytes(path)
            path = str(raw)
        elif path is None:
            mode, path = "none", ""
        elif isinstance(dir_fd, int):
            mode, fds = "fd", (dir_fd,)
        elif dir_fd is not None:
            mode = "bad-fd"
        argv = (mode, path, str(int(ignore_errors)), str(dir_fd))
        calls += 1
        if onerror is not None or onexc is not None:
            assert callback is not None
            lines = rmtree_handlers.run(callback, argv, fds, onerror, onexc, events)
        else:
            lines = call(*argv, pass_fds=fds)
        if lines != ["ok"]:
            raise exception(lines)

    remove.avoids_symlink_attacks = call("safe") == ["True"]
    calls = 0
    def adapter(case, original):
        nonlocal calls
        name = case._testMethodName
        if name in BLOCKERS:
            return {"status": "blocked", "reason": BLOCKERS[name]}
        if name in CALLBACKS and callback is None:
            return {"status": "blocked", "reason": "the original host callback bridge requires a Linux linker with --wrap"}
        events.clear()
        calls = 0
        with patch.object(suite, "shutil", SimpleNamespace(rmtree=remove)):
            original()
        if name in CALLBACKS:
            assert calls == CALLBACKS[name] and events, (case.id(), calls, events)
            row = {"status": "mixed", "native_calls": calls, "callbacks": list(events),
                   "detail": "native traversal, handler selection and errors; CPython callback and OS function objects; onerror traceback from host bridge"}
        else:
            assert calls == CASES[name], (case.id(), calls)
            row = {"status": "host_driven_native", "native_calls": calls}
        if with_roles:
            return {"roles": roles(body="cpython", assertions="cpython", fixture="cpython",
                                   operation="mixed" if events else "imre"),
                    "evidence": dict(row, operation="shutil.rmtree", opt_level=level,
                                     fixture="CPython TestRmTree creates the original files, permissions and assertions; handler and OS function objects remain in the host when present")}
        return row

    # A readable directory need not be searchable. Its open scan descriptor
    # suffices to list it, and removal requires write permission on the parent.
    restricted = folder / "readable-empty-directory"
    restricted.mkdir(mode=0o600)
    calls = 0
    remove(str(restricted))
    assert calls == 1 and not restricted.exists()
    if callback is not None:
        def reject(*args):
            raise AssertionError("ignore_errors invoked a callback")
        remove(str(folder / "missing"), True, reject, onexc=reject)
        def reraises(op, path, error):
            assert op is os.lstat and error is sys.exception() and error.filename == path
            raise
        try:
            remove(str(folder / "missing"), onexc=reraises)
        except FileNotFoundError as exc:
            assert exc.filename == str(folder / "missing")
        else:
            raise AssertionError("the exception raised by the callback was lost")

    return {suite.TestRmTree(name).id(): adapter
            for name in unittest.defaultTestLoader.getTestCaseNames(suite.TestRmTree)
            if not with_roles or name not in BLOCKERS}


def execute(args, suite, folder, level):
    executions = executors(args, suite, folder, level)
    evidence, failures, skips = run_in_fixtures(load_suite("test.test_shutil"), executions)
    rows = {name: value[0] for name, value in evidence.items() if value}
    rows.update({name: {"status": "reference_skip", "reason": reason} for name, reason in skips})
    rows.update({name: {"status": "failed", "diagnostics": reason} for name, reason in failures})
    assert rows.keys() == executions.keys()
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--"+name, required=True, type=lambda p: Path(p).resolve())
    parser.add_argument("--cpython-source", type=Path)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5) or os.name != "posix":
        parser.error("requires test-enabled CPython 3.14.5 on POSIX")
    if args.cpython_source:
        sys.path.insert(0,str(args.cpython_source.resolve() / "Lib"))
    manifest = json.loads((Path(__file__).resolve().parent / "UPSTREAM.json").read_text())
    verify_files(STDLIB / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    import test
    test_root = STDLIB / "upstream/Lib/test"
    test.__path__.insert(0,str(test_root))
    report = {"cpython_commit": manifest["commit"], "optimization_levels": {}}
    with tempfile.TemporaryDirectory() as tmp, chdir(tmp):
        from test import test_shutil as suite
        assert Path(suite.__file__).resolve() == test_root / "test_shutil.py"
        for level in (0, 3):
            folder = Path(tmp) / str(level)
            folder.mkdir()
            with chdir(folder):
                report["optimization_levels"][level] = execute(args,suite,folder,level)
    args.result.write_text(json.dumps(report,indent=2)+"\n")
    return int(any(r["status"] == "failed" for rows in report["optimization_levels"].values() for r in rows.values()))

if __name__ == "__main__":
    sys.exit(main())
