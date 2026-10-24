"""Run complete CPython os methods, routing getcwd and fspath to native Imre."""
import argparse
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import types
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from upstream import verify_upstream


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--cpython-source", type=Path)
    parser.add_argument("--opt-level", choices=("0", "1", "2", "3"), default="3")
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("use CPython 3.14.5 for the original unittest assertions")
    verify_upstream()
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    import test
    vendored = HERE / "upstream/Lib/test"
    test.__path__.insert(0, str(vendored))
    from test import test_os
    assert Path(test_os.__file__).resolve() == vendored / "test_os.py"

    with tempfile.TemporaryDirectory() as tmp:
        binary = Path(tmp) / "os-driver"
        driver = HERE / "os_driver.py"
        subprocess.run([args.imre.resolve(), "build", driver, "--no-cache", "--opt-level",
                        args.opt_level, "--clang", args.clang.resolve(), "--runtime",
                        args.runtime.resolve(), "-o", binary], cwd=tmp, check=True)

        def invoke(*args):
            output = subprocess.check_output([binary, *args], timeout=30)
            if not output.endswith(b"\n"):
                raise ValueError(f"incomplete native output: {output!r}")
            return output[:-1]

        def native_fspath(path):
            if isinstance(path, bytes):
                return invoke("bytes", path)
            return invoke("str", path).decode()

        # The upstream long-path fixture owns chdir/mkdir and cleanup. Replace
        # just this test module's os binding so its every getcwd runs natively.
        os_view = types.ModuleType("os")
        os_view.__dict__.update(vars(os))
        os_view.getcwd = lambda: invoke("cwd").decode()
        suite = unittest.TestSuite(test_os.MiscTests(name) for name in
                                   ("test_getcwd", "test_getcwd_long_path"))
        for name in ("test_return_bytes", "test_return_string"):
            case = test_os.TestPEP519(name)
            case.fspath = native_fspath
            suite.addTest(case)
        with patch.object(test_os, "os", os_view):
            result = unittest.TextTestRunner(verbosity=2).run(suite)
        if not result.wasSuccessful() or result.skipped or result.testsRun != 4:
            return 1

        # Both processes inherit a directory that the parent unlinks before exec.
        def removed_output(command):
            directory = Path(tempfile.mkdtemp(dir=tmp))
            return subprocess.check_output(command, cwd=directory,
                                           preexec_fn=directory.rmdir, timeout=30)
        reference = removed_output([sys.executable, driver, "cwd"])
        actual = removed_output([binary, "cwd"])
        if actual != reference or b"No such file or directory" not in actual:
            raise ValueError(f"deleted cwd: native {actual!r}, CPython {reference!r}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
