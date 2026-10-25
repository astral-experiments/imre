"""Exercise the complete, unchanged CPython TestCopy class at both optimization levels."""
import ast
import builtins
import errno
import os
from pathlib import Path
import sys
from types import SimpleNamespace
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import copyfile
from copyfile import ERRORS, build, run
from suite_inventory import cases as upstream_cases, load_suite
from suite_evidence import roles
from suite_native import NativeCompileError, Toolchain, native_method, run_in_fixtures

HOST_CASES = {
    "test_copy": 1, "test_copy2": 1, "test_copy_dir": 3, "test_copy2_dir": 3,
    "test_copy_symlinks": 2, "test_copy2_symlinks": 2, "test_copy_return_value": 4,
    "test_copy2_xattr": 1, "test_copyxattr_symlinks": 2,
    "test_copymode_follow_symlinks": 4, "test_copymode_symlink_to_symlink": 3,
    "test_copymode_symlink_to_symlink_wo_lchmod": 1,
    "test_copystat_symlinks": 2 + hasattr(os, "lchmod"),
}
MOCK_CASES = {
    "test_copystat_handles_harmless_chflags_errors": "patches os.chflags in the host process",
    "test_copyxattr": "replaces os.setxattr and os.listxattr in the host process",
}
OPERATIONS = ("copy", "copy2", "copyfile", "copymode", "copystat", "_copyxattr")


def driver():
    branches = "".join(
        f"    if sys.argv[1] == '{op}':\n"
        f"        result_{op} = shutil.{op}(sys.argv[2], sys.argv[3], follow_symlinks=sys.argv[4] == '1')\n"
        f"        print('ok')\n        print(repr(result_{op}))\n" for op in OPERATIONS)
    return "import shutil\nimport sys\ntry:\n" + branches + "".join(
        f"except {exc} as error:\n    print('{exc}')\n    print(repr(str(error)))\n" for exc in ERRORS)


def return_value_body(tools, folder, suite, case):
    # The original loop makes two independent source/destination pairs.
    paths = [case.mkdtemp() for _ in range(4)]
    fixture = f"""import os.path
import shutil

def create_file(path: str, content: str) -> None:
    with open(path, 'xb') as stream:
        stream.write(content.encode())

class NativeCase:
    index: int
    def __init__(self) -> None:
        self.index = 0
    def mkdtemp(self) -> str:
        self.index += 1
        return {paths!r}[self.index - 1]
    def assertEqual(self, actual: str, expected: str) -> None:
        assert actual == expected
"""
    return native_method(tools, folder, Path(suite.__file__), "TestCopy", case.id(), fixture,
                         operation=("imre", "shutil.copy and shutil.copy2"))


def executors(args, suite, directory, level, *, with_roles=False):
    exe = build(args, directory, "metadata", driver().encode(), level)
    errors = {n: getattr(suite.shutil if "." in n else builtins, n.split(".")[-1]) for n in ERRORS}

    def host(case, original):
        name = case._testMethodName
        if name in MOCK_CASES:
            return {"status": "blocked", "reason": MOCK_CASES[name]}
        method_blocker = {}
        if with_roles and name == "test_copy_return_value":
            folder = directory / name
            folder.mkdir()
            tools = Toolchain(args.imre, args.runtime, args.clang, level)
            try:
                execute = return_value_body(tools, folder, suite, case)
            except NativeCompileError as exc:
                message = str(exc).split("Imre: ", 1)[-1].strip()
                if message != "name is not a local value; globals and first-class functions are unsupported":
                    raise
                method_blocker["native_body_blocker"] = (
                    "for fn in (shutil.copy, shutil.copy2): " + message)
            else:
                return execute(case, original)
        counts = {}
        def adapter(op):
            def invoke(src, dst, *, follow_symlinks=True):
                result = run([exe, op, src, dst, str(int(follow_symlinks))])
                if result.stderr:
                    raise AssertionError(result.stderr)
                status, payload = result.stdout.rstrip("\n").split("\n", 1)
                value = ast.literal_eval(payload)
                counts[op] = counts.get(op, 0) + 1
                if status != "ok":
                    raise errors[status](value)
                return value
            return invoke
        with patch.object(suite, "shutil", SimpleNamespace(**{op: adapter(op) for op in OPERATIONS})):
            original()
        calls = sum(counts.values())
        assert calls == HOST_CASES[name], (case.id(), calls)
        if with_roles:
            return {"roles": roles(body="cpython", assertions="cpython", operation="imre", fixture="cpython"),
                    "evidence": {"operation": ", ".join("shutil." + op for op in counts),
                                 "native_calls": calls, "calls_by_operation": counts, "opt_level": level, **method_blocker,
                                 "fixture": "original TestCopy temporary paths, filesystem assertions, and cleanup in CPython"}}
        return {"status": "host_driven_native", "native_calls": calls}

    # The upstream decorator also needs trusted attributes. Exercise ordinary
    # user attributes independently on filesystems where that narrower probe works.
    if hasattr(os, "setxattr"):
        src, dst = directory / "xattr-in", directory / "xattr-out"
        src.write_bytes(b"attr")
        try:
            os.setxattr(src, "user.shutil_probe", bytes(range(256)) * 4)
        except OSError as error:
            if error.errno not in (errno.ENOTSUP, errno.EPERM, errno.EACCES):
                raise
        else:
            run([exe, "copy2", src, dst, "1"])
            assert os.getxattr(src, "user.shutil_probe") == os.getxattr(dst, "user.shutil_probe")
    names = HOST_CASES.keys() if with_roles else HOST_CASES.keys() | MOCK_CASES.keys()
    return {suite.TestCopy(name).id(): host for name in names}


def cases(args, suite, directory, level):
    rows = copyfile.cases(args, suite, directory, level)
    full = load_suite("test.test_shutil")
    expected = {case.id() for case in upstream_cases(full) if isinstance(case, suite.TestCopy)}
    selected = executors(args, suite, directory, level)
    evidence, failures, skips = run_in_fixtures(full, selected)
    rows.update({name: values[0] for name, values in evidence.items() if values})
    rows.update({name: {"status": "failed", "diagnostics": reason} for name, reason in failures})
    rows.update({name: {"status": "reference_skip", "reason": reason} for name, reason in skips})
    assert rows.keys() == expected
    return rows


if __name__ == "__main__":
    sys.exit(copyfile.main(cases))
