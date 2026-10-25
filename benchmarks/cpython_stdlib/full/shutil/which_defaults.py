"""Transfer the original missing-PATH fixture's os attributes to their native call sites."""
import ast
from contextlib import chdir
import errno
import os
from pathlib import Path
from types import SimpleNamespace
import socket
import struct
import subprocess
from unittest.mock import patch

DRIVER = '''import shutil
import sys

try:
    if sys.argv[1] == "bytes":
        with open(sys.argv[2], "rb") as source:
            print(repr(shutil.which(source.read())))
    elif sys.argv[1] == "explicit":
        print(repr(shutil.which(sys.argv[2], path=sys.argv[3])))
    else:
        print(repr(shutil.which(sys.argv[2])))
except OSError as exc:
    print("raised " + repr(exc))
except RuntimeError as exc:
    print("raised " + repr(exc))
'''


def build(tools, folder):
    source = folder / "defaults.py"
    source.write_text(DRIVER)
    obj, binary = folder / "defaults.o", folder / "defaults"
    subprocess.run([tools.clang, "-std=c11", "-Wall", "-Wextra", "-Werror", "-c",
                    Path(__file__).with_suffix(".c"), "-o", obj], check=True)
    symbols = ("imre_shutil_confstr_cs_path", "imre_shutil_os_defpath", "confstr")
    flags = [arg for symbol in symbols for arg in ("-C", "link-arg=-Wl,--wrap=" + symbol)]
    subprocess.run([tools.imre, "build", source, "-o", binary, "--runtime", tools.runtime,
                    "--clang", tools.clang, "--opt-level", str(tools.level), "--no-cache",
                    *flags, "-C", f"link-arg={obj}"], check=True, capture_output=True, timeout=180)
    return binary


def invoke(binary, cmd, trace, *, explicit=None):
    if isinstance(cmd, bytes):
        raw = binary.parent / "command.bin"
        raw.write_bytes(cmd)
        args = ("bytes", raw)
    else:
        args = ("text", cmd) if explicit is None else ("explicit", cmd, explicit)
    parent, child = socket.socketpair()
    with parent, child:
        parent.settimeout(30)
        env = dict(os.environ, IMRE_WHICH_FIXTURE_FD=str(child.fileno()))
        process = subprocess.Popen([binary, *args], env=env, pass_fds=(child.fileno(),),
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        child.close()
        try:
            with parent.makefile("rb") as reader:
                while request := reader.read(2):
                    key, path_present = request
                    assert not path_present and "PATH" not in env
                    assert key in (ord("C"), ord("D"))
                    name = 'os.confstr("CS_PATH")' if key == ord("C") else "os.defpath"
                    try:
                        value = os.confstr("CS_PATH") if key == ord("C") else os.defpath
                    except (AttributeError, ValueError, RuntimeError) as exc:
                        tag = {AttributeError: 14, ValueError: 8, RuntimeError: 10}[type(exc)]
                        assert not exc.args or len(exc.args) == 1 and isinstance(exc.args[0], str)
                        data = exc.args[0].encode() if exc.args else b""
                        length = len(data) if exc.args else 0xffffffff
                        outcome = type(exc).__name__
                    else:
                        tag = 0 if value is None else 1
                        data = b"" if value is None else os.fsencode(value)
                        length, outcome = len(data), type(value).__name__
                    trace.append((name, outcome))
                    parent.sendall(struct.pack("=II", tag, length) + data)
            stdout, stderr = process.communicate(timeout=30)
            if process.returncode or stderr:
                raise AssertionError((process.returncode, stdout, stderr))
            if stdout.startswith("raised "):
                return stdout.strip()
            return ast.literal_eval(stdout)
        finally:
            if process.poll() is None:
                process.kill()
            process.wait()


def controls(binary):
    """Cover no setting, ordinary system failure, and branches that never consult defaults."""
    env = dict(os.environ)
    env.pop("PATH", None)
    for fault, expected in (("None", "None"), ("EIO", "raised " + repr(OSError(errno.EIO, os.strerror(errno.EIO))))):
        actual = subprocess.run([binary, "text", "sh"], env=dict(env, IMRE_WHICH_FIXTURE_CONFSTR=fault),
                                check=True, capture_output=True, text=True)
        assert (actual.stdout.strip(), actual.stderr) == (expected, "")
    trace = []
    with patch.dict(os.environ):
        os.environ.pop("PATH", None)
        with patch.object(os, "confstr", side_effect=AttributeError("missing")):
            result = invoke(binary, "sh", trace)
            assert result and Path(result).is_file()
            assert trace == [('os.confstr("CS_PATH")', "AttributeError"), ("os.defpath", "str")]
        trace.clear()
        with patch.object(os, "confstr", side_effect=RuntimeError("from confstr")):
            assert invoke(binary, "sh", trace) == "raised RuntimeError('from confstr')"
            assert trace == [('os.confstr("CS_PATH")', "RuntimeError")]
        trace.clear()
        assert invoke(binary, "sh", trace, explicit="") is None
        assert invoke(binary, "/bin/sh", trace) == "/bin/sh"
        os.environ["PATH"] = ""
        assert invoke(binary, "sh", trace) is None
        assert not trace


def observation(program, case, original, suite, level, count_call=None):
    from suite_evidence import roles
    trace = []
    calls = 0
    def which(cmd):
        nonlocal calls
        calls += 1
        if count_call:
            count_call()
        return invoke(program, cmd, trace)
    with patch.object(suite, "shutil", SimpleNamespace(which=which)):
        original()
    kind = type(case.file).__name__
    assert calls == 2 and trace == [
        ('os.confstr("CS_PATH")', "ValueError"), ("os.defpath", kind),
        ('os.confstr("CS_PATH")', kind)], (calls, trace)
    return {"roles": roles(body="cpython", assertions="cpython", operation="mixed", fixture="cpython"),
            "evidence": {"operation": "native shutil.which reads PATH and searches executable files; original host OS providers",
                         "native_calls": calls, "opt_level": level, "providers": trace,
                         "native_environment": {"PATH": "absent", "processes": 2},
                         "fixture": "original EnvironmentVarGuard removed PATH in both native children; unittest.mock.patch and swap_attr supply os.confstr and os.defpath"}}


def paired(args, suite, work):
    from suite_inventory import load_suite
    from suite_native import Toolchain, run_in_fixtures
    ids = {cls("test_environ_path_missing").id() for cls in (suite.TestWhich, suite.TestWhichBytes)}
    levels = {}
    for level in (0, 3):
        folder = work / f"missing-PATH-{level}"
        folder.mkdir()
        tools = Toolchain(args.imre.resolve(), args.runtime.resolve(), args.clang.resolve(), level)
        program = build(tools, folder)
        controls(program)
        def execute(case, original):
            return observation(program, case, original, suite, level)
        with chdir(folder):
            rows, failures, skips = run_in_fixtures(load_suite("test.test_shutil"), dict.fromkeys(ids, execute))
        if failures or skips or set(rows) != ids:
            raise AssertionError((failures, skips, set(rows), ids))
        levels[level] = {name: values[0]["evidence"] for name, values in rows.items()}
    return levels
