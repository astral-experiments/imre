"""Run original move methods, including their actual cross-device rename mock."""

import ast
import builtins
from contextlib import contextmanager
import errno
import hashlib
import os
from pathlib import Path
import shutil
import socket
import struct
import subprocess
import sys
from threading import Thread
from types import SimpleNamespace
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import tree_ops
import tree_host_callbacks
from suite_evidence import roles
from suite_inventory import cases, load_suite
from suite_native import NativeCompileError, method_bytes, native_method, run_in_fixtures

NATIVE_METHODS = {
    "test_move_file": "_check_move_file", "test_move_file_to_dir": "_check_move_file",
    "test_move_dir_to_dir": "_check_move_dir", "test_move_dir_sep_to_dir": "_check_move_dir",
    "test_move_return_value": None, "test_move_as_rename_return_value": None,
}

COPY_CALLBACKS = {"test_move_file_special_function": 1, "test_move_dir_special_function": 3}
CALLBACK_DRIVER = "import shutil\nimport sys\n\n" + tree_host_callbacks.COPY_CALLBACK + '''
try:
    moved = shutil.move(sys.argv[1], sys.argv[2], copy_function=copy_file)
    if moved is None:
        raise AssertionError("distinct move destinations must return a pathname")
    print(moved)
except RuntimeError as exc:
    print(str(exc))
'''


def native_fixture(case, source, helper):
    fields = {k: getattr(case, k) for k in ("src_dir", "dst_dir", "src_file", "dst_file")}
    fixture = ("import os\nimport os.path\nimport shutil\nimport unittest\n\nclass NativeCase:\n"
               + "".join(f"    {key}: str\n" for key in fields)
               + "    def __init__(self) -> None:\n"
               + "".join(f"        self.{key} = {v!r}\n" for key, v in fields.items()))
    fixture += ("    def assertFalse(self, condition: bool) -> None:\n"
                "        unittest.TestCase().assertFalse(condition)\n")
    if helper == "_check_move_file":
        fixture += ("    def assertEqual(self, first: bytes, second: bytes) -> None:\n"
                    "        unittest.TestCase().assertTrue(first == second)\n")
    elif helper == "_check_move_dir":
        fixture += ("    def assertEqual(self, first: list[str], second: list[str]) -> None:\n"
                    "        unittest.TestCase().assertTrue(first == second)\n")
    else:
        fixture += ("    def assertEqual(self, first: str | None, second: str) -> None:\n"
                    "        if first is None:\n"
                    "            unittest.TestCase().fail('move unexpectedly returned None')\n"
                    "        else:\n"
                    "            unittest.TestCase().assertMultiLineEqual(first, second)\n")
    if helper:
        body = method_bytes(source, "TestMove", helper)
        return fixture + body.decode(), hashlib.sha256(body).hexdigest()
    return fixture, None


def driver():
    source = '''import os.path
import shutil
import sys
from pathlib import Path
try:
    if sys.argv[1] == "inside":
        print(repr(shutil._destinsrc(sys.argv[2], sys.argv[3])))
    if sys.argv[1] == "rmtree":
        shutil.rmtree(sys.argv[2], ignore_errors=True)
        print("None")
'''
    for s in (False, True):
        for d in (False, True):
            src, dst = ("Path(sys.argv[2])" if s else "sys.argv[2]", "Path(sys.argv[3])" if d else "sys.argv[3]")
            source += (f'    if sys.argv[1] == "move{int(s)}{int(d)}":\n'
                       f'        print(repr(shutil.move({src}, {dst})))\n')
    for cls in ("shutil.Error", "OSError", "ValueError", "TypeError"):
        source += f'except {cls} as e:\n    print("{cls}")\n    print(repr(e))\n    print(repr(str(e)))\n'
        if cls in ("shutil.Error", "OSError"):
            source += '    print(repr(e.filename))\n    print(repr(e.filename2))\n'
    return source


def decode_result(output):
    lines = output.splitlines()
    if len(lines) == 1:
        value = lines[0]
        if value.startswith("PosixPath("):
            return Path(ast.literal_eval(value[10:-1]))
        return ast.literal_eval(value)
    catch, representation, message, *paths = lines
    constructor = ast.parse(representation, mode="eval").body
    assert isinstance(constructor, ast.Call) and isinstance(constructor.func, ast.Name) and not constructor.keywords
    name = constructor.func.id
    cls = (getattr(shutil, name) if name in {"Error", "SameFileError", "SpecialFileError"} else getattr(builtins, name))
    caught = shutil.Error if catch == "shutil.Error" else getattr(builtins, catch)
    assert issubclass(cls, caught)
    error = cls(*(ast.literal_eval(arg) for arg in constructor.args))
    if isinstance(error, OSError):
        filename, filename2 = map(ast.literal_eval, paths)
        if filename is not None:
            error.filename = filename
        if filename2 is not None:
            error.filename2 = filename2
    else:
        assert not paths
    assert str(error) == ast.literal_eval(message), (error, message)
    raise error


@contextmanager
def rename_mock(mock, shim):
    # Only the native child loads the shim. The parent executes the very
    # function installed by the upstream decorator with the native path pair.
    parent, child = socket.socketpair()
    child.set_inheritable(True)
    calls, errors = [], []
    def answer():
        with parent, parent.makefile("rb") as stream:
            def path():
                value = bytearray()
                while (b := stream.read(1)) not in (b"", b"\0"):
                    value += b
                return os.fsdecode(bytes(value)) if b else None
            while (src := path()) is not None:
                dst = path()
                if dst is None:
                    errors.append("partial rename request")
                    return
                number = 0
                try:
                    mock(src, dst)
                except OSError as exc:
                    number = exc.errno
                except Exception as exc:
                    errors.append(str(exc))
                    number = errno.EIO
                calls.append((src, dst, number))
                parent.sendall(struct.pack("=i", number))
    server = Thread(target=answer)
    env = dict(os.environ, LD_PRELOAD=str(shim), SHUTIL_TEST_RENAME_FD=str(child.fileno()))
    with child:
        server.start()
        try:
            yield env, (child.fileno(),), calls
        finally:
            child.close()
            server.join(timeout=5)
    assert not server.is_alive() and not errors, errors
    assert all(n == errno.EXDEV for _, _, n in calls), calls


def executors(module, tools, work, *, with_roles=False):
    exe = tools.build(driver().encode(), work)
    callbacks = work / "move-callback"
    callbacks.mkdir()
    callback_exe = tools.build(CALLBACK_DRIVER.encode(), callbacks)
    shim = work / "mock_rename.so"
    if sys.platform.startswith("linux"):
        subprocess.run([tools.clang, "-shared", "-fPIC", "-Wall", "-Werror",
                        tree_ops.HERE / "rename_mock.c", "-o", shim], check=True)

    def adapter(case, original):
        name = case._testMethodName
        native_blocker = {}
        if name in NATIVE_METHODS:
            source = Path(module.__file__)
            fixture, helper_sha = native_fixture(case, source, NATIVE_METHODS[name])
            folder = work / name
            folder.mkdir()
            try:
                execute = native_method(tools, folder, source, "TestMove", case.id(), fixture,
                                        operation=("imre", "shutil.move"))
            except NativeCompileError as exc:
                # A compile failure does not mutate the real fixture. Preserve the
                # diagnostic and still run the original body with native operations.
                native_blocker["native_body_blocker"] = str(exc).strip().splitlines()[-1].split("Imre: ")[-1]
            else:
                row = {"status": "native_body_host_fixture", **execute(case, original)}
                row["observation"]["evidence"]["assertion_fixture"] = "typed equality on the original operands, using native unittest assertions"
                if helper_sha:
                    row["helper_sha256"] = helper_sha
                    row["observation"]["evidence"]["helper_sha256"] = helper_sha
                return row
        ops, interceptions, host_paths = {}, 0, 0
        events = []
        def call(op, src, dst, callback=None):
            nonlocal interceptions
            argv = [op, src, dst]
            if op.startswith("move") and os.rename is module._fake_rename:
                if not shim.exists():
                    raise NotImplementedError("upstream rename interposition needs a platform adapter")
                with rename_mock(os.rename, shim) as (env, fds, calls):
                    if callback is None:
                        child = subprocess.run([exe, *argv], env=env, pass_fds=fds, text=True,
                                               capture_output=True, check=True, timeout=60)
                        assert not child.stderr, child.stderr
                        out = child.stdout
                    else:
                        # These two original methods move into an existing directory.
                        assert os.path.isdir(dst)
                        target = os.path.join(dst, os.path.basename(src.rstrip(os.sep)))
                        output = tree_host_callbacks.call(callback_exe, src, dst, callback, events,
                                                         env=env, pass_fds=fds, expected_dst=target)
                        out = repr(output) + "\n"
                if not calls:
                    assert out.startswith("shutil.Error\n") and "already exists" in out, out
                interceptions += len(calls)
            else:
                assert callback is None
                result = subprocess.run([exe, *argv], text=True, capture_output=True, check=True, timeout=60)
                assert not result.stderr, result.stderr
                out = result.stdout
            key = "move" if op.startswith("move") else op
            ops[key] = ops.get(key, 0) + 1
            return decode_result(out)
        def move(src, dst, copy_function=None):
            nonlocal host_paths
            host_paths += sum(not isinstance(p, (str, bytes, Path)) for p in (src, dst))
            return call(f"move{int(isinstance(src, Path))}{int(isinstance(dst, Path))}", src, dst, copy_function)
        def rmtree(src, ignore_errors=False):
            assert ignore_errors
            return call("rmtree", src, "")
        proxy = SimpleNamespace(move=move, _destinsrc=lambda s, d: call("inside", s, d),
                                rmtree=rmtree, Error=module.Error)
        with patch.object(module, "shutil", proxy):
            original()
        assert ops.get("move", 0) + ops.get("inside", 0), ops
        detail = {}
        if name in COPY_CALLBACKS:
            assert len(events) == COPY_CALLBACKS[name] and all(e == ["str", "str"] for e in events)
            detail["copy_callback_argument_types"] = events
        if interceptions:
            detail.update(native_rename_calls=interceptions, mock="original CPython _fake_rename returned EXDEV; Imre performed the fallback")
        if host_paths:
            detail.update(host_pathlike_conversions=host_paths, host="CPython called FakePath.__fspath__; Imre moved the resulting path")
        if with_roles:
            return {"roles": roles(body="cpython", assertions="cpython", operation="mixed" if detail else "imre", fixture="cpython"),
                    "evidence": dict(operation=", ".join("shutil." + ("_destinsrc" if op == "inside" else op) for op in ops),
                                     calls_by_operation=ops, native_calls=sum(ops.values()), opt_level=tools.level,
                                     fixture="original CPython move setup, assertions and cleanup", **detail)}
        return {"status": "mixed" if detail else "host_driven_native", "operations": ops, **detail, **native_blocker}

    suite = load_suite("test.test_shutil")
    return {case.id(): adapter for case in cases(suite) if isinstance(case, module.TestMove)}


def execute_level(module, tools, work):
    executions = executors(module, tools, work)
    suite = load_suite("test.test_shutil")
    evidence, failures, skips = run_in_fixtures(suite, executions)
    rows = {name: values[0] for name, values in evidence.items() if values}
    rows.update({name: {"status": "failed", "reason": msg} for name, msg in failures})
    rows.update({name: {"status": "reference_skip", "reason": msg} for name, msg in skips})
    assert rows.keys() == executions.keys()
    return rows


if __name__ == "__main__":
    sys.exit(tree_ops.main(execute_level))
