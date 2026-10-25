"""Run descriptor checks that require a host file object or independent fcntl."""
import argparse
from contextlib import contextmanager
import ctypes
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import types
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from native_descriptors import HERE, verify_sources

CASES = {
    "test_os": ("FDInheritanceTests.test_get_inheritable_cloexec",
                "FDInheritanceTests.test_set_inheritable_cloexec",
                "FDInheritanceTests.test_get_set_inheritable_o_path"),
    "test_posix": ("PosixTester.test_dup", "PosixTester.test_dup2"),
}


@contextmanager
def bindings(library):
    # Open the capture and its saved descriptor before any test allocates fds.
    # Each call enters generated main against the test process's descriptor
    # table. Host fcntl can observe the same descriptor before and after a call.
    entry = ctypes.CDLL(str(library)).main
    entry.argtypes = (ctypes.c_int, ctypes.POINTER(ctypes.c_char_p))
    entry.restype = ctypes.c_int
    calls = []
    with tempfile.TemporaryFile() as output:
        saved = os.dup(1)
        try:
            def invoke(name, *args):
                calls.append(name)
                encoded = [b"os-driver", name.encode(), *(str(arg).encode() for arg in args)]
                argv = (ctypes.c_char_p * (len(encoded) + 1))(*encoded, None)
                output.seek(0)
                output.truncate()
                os.dup2(output.fileno(), 1)
                try:
                    status = entry(len(encoded), argv)
                finally:
                    os.dup2(saved, 1)
                if status:
                    raise RuntimeError(f"native {name} exited with status {status}")
                output.seek(0)
                return output.read()

            # Only the audited operations are exposed. A new OS call in a test
            # cannot silently run on CPython. Constants describe the host ABI.
            view = types.SimpleNamespace(
                O_RDONLY=os.O_RDONLY,
                open=lambda path, flags, mode=0o777: int(invoke("open", path, flags, mode)),
                close=lambda fd: void(invoke("close", fd)),
                dup=lambda fd: int(invoke("dup", fd)),
                dup2=lambda fd, fd2, inheritable=True: int(invoke("dup2", fd, fd2, int(inheritable))),
                get_inheritable=lambda fd: {b"True\n": True, b"False\n": False}[invoke("get_inheritable", fd)],
                set_inheritable=lambda fd, value: void(invoke("set_inheritable", fd, int(value))),
            )
            if hasattr(os, "O_PATH"):
                view.O_PATH = os.O_PATH
            yield view, calls
        finally:
            os.close(saved)


def void(data):
    if data:
        raise ValueError(f"unexpected native return data: {data!r}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--cpython-source", type=Path)
    parser.add_argument("--opt-level", choices=("0", "3"), required=True)
    args = parser.parse_args()
    modules, commit = verify_sources(args.cpython_source)
    report = {}
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        llvm = work / "calls.ll"
        library = work / "calls.so"
        runtime = args.runtime.resolve()
        clang = args.clang.resolve()
        subprocess.run([args.imre.resolve(), "build", HERE / "descriptor_calls.py",
                        "--whole-program", "--no-cache", "--opt-level", args.opt_level,
                        "--emit-llvm", llvm, "--runtime", runtime, "--clang", clang,
                        "-o", work / "calls"], check=True, cwd=work, timeout=120)
        # Link the unmodified emitted LLVM as a library so the unchanged test
        # and the compiled calls share an fd table. No CPython API is linked.
        subprocess.run([clang, "-O" + args.opt_level, "-fPIC", "-shared", llvm, runtime,
                        "-ldl", "-lpthread", "-lm", "-o", library], check=True, cwd=work, timeout=120)
        before = Path.cwd()
        try:
            os.chdir(work)
            with bindings(library) as (view, calls):
                for name, selections in CASES.items():
                    mod = modules[name]
                    for selection in selections:
                        cls, method = selection.split(".")
                        case = getattr(mod, cls)(method)
                        calls.clear()
                        with patch.object(mod, "os", view), patch.object(mod, "posix", view, create=True):
                            result = unittest.TestResult()
                            case.run(result)
                        if result.skipped:
                            row = {"reference_skip": result.skipped[0][1]}
                        elif not result.wasSuccessful() or not calls:
                            row = {"status": "failed", "diagnostic": str(result.failures + result.errors)}
                        else:
                            row = {"status": "host_driven_native", "native_calls": list(calls)}
                        report[case.id()] = row
                        print(case.id(), row, flush=True)
        finally:
            os.chdir(before)
    args.result.write_text(json.dumps({"cpython_commit": commit, "opt_level": int(args.opt_level),
                                      "cases": report}, indent=2) + "\n")
    return int(any(row.get("status") == "failed" for row in report.values()))


if __name__ == "__main__":
    sys.exit(main())
