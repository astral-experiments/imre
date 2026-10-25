"""Observe the sys.thread_info reads made by the unchanged CPython parent."""
import ast
import os
from pathlib import Path
import subprocess
from unittest.mock import patch

from suite_evidence import roles


def lines(binary, operation, *, env=None):
    result = subprocess.run([binary, operation], check=True, capture_output=True, text=True, timeout=15, env=env)
    if result.stderr:
        raise AssertionError(result.stderr)
    return result.stdout.splitlines()


def check_record(binary, clang):
    """Check identity, slicing, names and the target libc independently of the upstream count."""
    imported, repeated, fields, names, sliced, representation = lines(binary, "describe")
    name, lock, version = ast.literal_eval(fields)
    assert imported == repeated == "True"
    assert lock is None and ast.literal_eval(sliced) == (name, lock, version)
    assert ast.literal_eval(names) == ("name", "lock", "version")
    assert representation == f"sys.thread_info(name={name!r}, lock={lock!r}, version={version!r})"
    if "CS_GNU_LIBPTHREAD_VERSION" in getattr(os, "confstr_names", {}):
        assert version == os.confstr("CS_GNU_LIBPTHREAD_VERSION")
        # The interposer aborts if two reads of the singleton query libc twice.
        shim = binary.parent / "thread-confstr.so"
        subprocess.run([clang, "-shared", "-fPIC", Path(__file__).with_name("thread_confstr.c"),
                        "-ldl", "-o", shim], check=True, timeout=30)
        preload = ":".join(filter(None, [os.environ.get("LD_PRELOAD"), str(shim)]))
        for supplied, expected in (("provider-12.34", "provider-12.34"), ("", None), ("x" * 256, None)):
            env = dict(os.environ, LD_PRELOAD=preload, IMRE_TEST_PTHREAD_VERSION=supplied)
            imported, repeated, fields, _, sliced, _ = lines(binary, "describe", env=env)
            assert imported == repeated == "True"
            assert ast.literal_eval(fields) == ast.literal_eval(sliced) == (name, None, expected)


def observer(module, binary, method_sha256, level):
    def execute(case, original):
        calls = []

        def read(field):
            line, = lines(binary, field)
            value = ast.literal_eval(line)
            calls.append(field)
            return value

        class Record:
            def __len__(self):
                return read("length")

            @property
            def name(self):
                return read("name")

            @property
            def lock(self):
                return read("lock")

        class NativeSys:
            @property
            def thread_info(self):
                value = read("thread_info")
                if not isinstance(value, tuple) or len(value) != 3:
                    raise AssertionError(value)
                return Record()

            @property
            def platform(self):
                return read("platform")

        with patch.object(module, "sys", NativeSys()):
            original()
        if calls[:4] != ["thread_info", "length", "name", "lock"] or "platform" not in calls:
            raise AssertionError(calls)
        return {
            "execution": "host-driven-native", "body": "CPython", "assertions": "CPython",
            "fixture_execution": "original CPython module/class/instance hooks",
            "calls": calls,
            "observation": {
                "roles": roles(body="cpython", assertions="cpython", operation="imre", fixture="cpython"),
                "evidence": {
                    "operation": "read the native thread record, its length and fields, and the target platform",
                    "fixture": "original SysModuleTest parent, case lifecycle and teardown",
                    "assertions": "original CPython TestCase equality and membership; platform branch selected from Imre",
                    "method_sha256": method_sha256, "native_calls": len(calls), "calls": calls, "opt_level": level,
                },
            },
        }

    return execute
