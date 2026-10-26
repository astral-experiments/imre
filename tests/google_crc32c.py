"""Run the unchanged upstream pytest suite against one chosen CRC32C backend."""

import argparse
import json
import os
import subprocess
import sys
from collections import Counter
from importlib.machinery import ExtensionFileLoader
from pathlib import Path
from types import ModuleType

import pytest


class ImreLeaf:
    """Forward only the two C methods. Upstream retains its own Checksum classes."""

    def __init__(self, binary):
        self.child = subprocess.Popen(
            [binary, "oracle"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True
        )
        assert self.child.stdout.readline() == "crc32c-ready\n"
        self.calls = Counter()

    def request(self, command):
        self.child.stdin.write(command + "\n")
        self.child.stdin.flush()
        result = self.child.stdout.readline()
        if not result:
            raise RuntimeError(f"native checksum worker stopped: {self.child.poll()}")
        return result.strip()

    def call(self, method, crc, data):
        self.calls[method] += 1
        return int(self.request(" ".join([method, str(crc), *map(str, data)])))

    def close(self):
        counts = tuple(map(int, self.request("quit").split()))
        self.child.stdin.close()
        assert self.child.wait(timeout=5) == 0
        assert counts == (self.calls["value"], self.calls["extend"])


class TestResults:
    def __init__(self):
        self.passed = 0

    def pytest_runtest_logreport(self, report):
        if report.when == "call" and report.passed:
            self.passed += 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("backend", choices=("cpython", "imre", "fallback"))
    parser.add_argument(
        "--upstream", type=Path, required=True, help="google-crc32c 1.8.0 source tree"
    )
    parser.add_argument(
        "--cpython-package", type=Path, help="root containing the CPython accelerator"
    )
    parser.add_argument(
        "--imre-binary", type=Path, help="compiled examples/google_crc32c/driver.py"
    )
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    os.environ["PYTEST_DISABLE_PLUGIN_AUTOLOAD"] = "1"
    sys.dont_write_bytecode = True
    package = (
        args.cpython_package if args.backend == "cpython" else args.upstream / "src"
    )
    if package is None or (args.backend == "imre" and args.imre_binary is None):
        parser.error("supply the built artifact for the selected backend")
    sys.path.insert(0, str(package.resolve()))
    bridge = None
    if args.backend == "imre":
        bridge = ImreLeaf(args.imre_binary.resolve())
        leaf = ModuleType("google_crc32c._crc32c")
        leaf.value = lambda data: bridge.call("value", 0, data)
        leaf.extend = lambda crc, data: bridge.call("extend", crc, data)
        sys.modules[leaf.__name__] = leaf

    import google_crc32c

    assert (
        Path(google_crc32c.__file__).resolve().parent
        == package.resolve() / "google_crc32c"
    )
    assert google_crc32c.implementation == (
        "python" if args.backend == "fallback" else "c"
    )
    if args.backend == "cpython":
        from google_crc32c import _crc32c

        assert isinstance(_crc32c.__loader__, ExtensionFileLoader)
        assert google_crc32c.value is _crc32c.value
        assert (
            Path(_crc32c.__file__).resolve().parent
            == package.resolve() / "google_crc32c"
        )
    if bridge:
        assert google_crc32c.value is leaf.value
        assert google_crc32c.extend is leaf.extend

    results = TestResults()
    options = [
        "-q",
        "-p",
        "no:cacheprovider",
        "--import-mode=importlib",
        str(args.upstream / "tests/test___init__.py"),
    ]
    if args.backend == "fallback":
        options += ["-k", "not cext"]
    assert pytest.main(options, plugins=[results]) == pytest.ExitCode.OK
    assert results.passed == (26 if args.backend == "fallback" else 42)
    outcome = {"backend": args.backend, "upstream_passed": results.passed}
    if bridge:
        assert bridge.calls["value"] > 0 and bridge.calls["extend"] > 0
        outcome["upstream_native_calls"] = dict(bridge.calls)
    if args.backend != "fallback":
        from google_crc32c import python

        # PyArg_ParseTuple's unsigned-long mask accepts negatives and large integers.
        for seed in [-1, 2**32 + 9, 2**100 + 9]:
            assert google_crc32c.extend(seed, b"xyz") == python.extend(
                seed & 0xFFFFFFFF, b"xyz"
            )
        outcome["unsigned_mask_cases"] = 3
    if bridge:
        bridge.close()
        outcome["native_counts_acknowledged"] = True
        public = subprocess.check_output(
            [args.imre_binary.resolve(), "public"], text=True
        )
        assert public == "public-api-ok\n"
        outcome["native_public_facade"] = "passed"
    print(json.dumps(outcome))
    if args.output:
        args.output.write_text(json.dumps(outcome, indent=2) + "\n")


if __name__ == "__main__":
    main()
