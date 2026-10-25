"""Measure unmodified linecache methods with native file reads and live cache state."""

import argparse
import ast
from collections import Counter
import hashlib
import json
from pathlib import Path
import select
import subprocess
import sys
import tempfile
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
sys.path.insert(0, str(HERE))
from filesystem import check_files, check_search
from suite_evidence import roles, write_reports
from suite_inventory import load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, method_bytes, run_evidence

SOURCE = HERE / "upstream/Lib/test/test_linecache.py"
MODULE = "test.test_linecache"
BAD = ("BadUnicode_NoDeclaration", "BadUnicode_WithDeclaration")
GOOD = ("EmptyFile", "SingleEmptyLine", "GoodUnicode")
HOST = {
    "LineCacheTests": (
        "test_getline",
        "test_no_ending_newline",
        "test_clearcache",
        "test_checkcache",
        "test_lazycache_no_globals",
    ),
    "LineCacheInvalidationTests": (
        "test_checkcache_for_deleted_file",
        "test_checkcache_for_modified_file",
        "test_checkcache_with_no_parameter",
    ),
    "SingleEmptyLine": ("test_getline",),
    "GoodUnicode": ("test_getline",),
}


class Calls:
    """A transport for native operations; the upstream test module keeps its own fixture."""

    def __init__(self, executable):
        self.process = subprocess.Popen(
            [executable],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        self.counts = Counter()
        self.cache = self

    def call(self, op, name="", *args):
        self.counts[op] += 1
        fields = (op, name, *map(str, args))
        assert not any("\n" in item for item in fields)
        self.process.stdin.write("\n".join(fields) + "\n")
        self.process.stdin.flush()
        if not select.select([self.process.stdout], [], [], 20)[0]:
            raise TimeoutError(f"native linecache: {op}")
        line = self.process.stdout.readline()
        if not line:
            raise AssertionError(self.process.stderr.read())
        for exc in (TypeError, LookupError, NotImplementedError):
            prefix = exc.__name__ + ":"
            if line.startswith(prefix):
                raise exc(line.removeprefix(prefix).removesuffix("\n"))
        return ast.literal_eval(line)

    def __contains__(self, key):
        return self.call("contains", key)

    def clearcache(self):
        return self.call("clear")

    def checkcache(self, filename=None):
        return self.call("all") if filename is None else self.call("check", filename)

    def getline(self, filename, lineno):
        return self.call(
            "float" if isinstance(lineno, float) else "line", filename, lineno
        )

    def getlines(self, filename):
        return self.call("lines", filename)

    def lazycache(self, filename, module_globals):
        assert module_globals is None
        return self.call("lazy", filename)

    def close(self):
        if self.process.poll() is None:
            self.process.stdin.write("quit\n")
            self.process.stdin.flush()
        try:
            stdout, stderr = self.process.communicate(timeout=10)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.communicate()
            raise
        if self.process.returncode or stdout or stderr:
            raise AssertionError((self.process.returncode, stdout, stderr))


def host_method(bridge, owner, method, level):
    # The original inherited test reads from tokenize.open and compares each
    # actual file line. An empty input has no operation and is never credited.
    definition = "GetLineTestsGoodData" if owner in GOOD else owner
    body = method_bytes(SOURCE, definition, method)

    def execute(case, original):
        before = bridge.counts.copy()
        original()
        observed = bridge.counts - before
        assert observed, case.id()
        return {
            "roles": roles(
                body="cpython",
                assertions="cpython",
                operation="imre",
                fixture="cpython",
            ),
            "evidence": {
                "operation": "unchanged body calls a persistent native linecache",
                "native_calls": dict(sorted(observed.items())),
                "opt_level": level,
                "method_sha256": hashlib.sha256(body).hexdigest(),
                "fixture": "Original CPython fixtures, real files and TestCase assertions; linecache operations and cache membership run in Imre.",
            },
        }

    return execute


def native_method(lang, directory, upstream, owner, method):
    if owner in BAD:
        definition = "GetLineTestsBadData"
    elif owner == "EmptyFile":
        definition = "EmptyFile"
    elif owner in GOOD:
        definition = "GetLineTestsGoodData"
    else:
        definition = owner
    body = method_bytes(SOURCE, definition, method)
    strings = owner in BAD and method == "test_getline"
    element = "str" if strings else "list[str]"
    fields = "    file_name: str\n    file_list: list[str]\n"
    if owner == "LineCacheTests":
        fields = ""
    fixture = f"""import linecache
import sys
from dataclasses import dataclass
FILENAME = sys.argv[1]

@dataclass
class NativeCase:
{fields}    def assertEqual(self, actual: {element}, expected: {element}) -> None:
        if len(actual) != len(expected):
            raise AssertionError("length differs")
        for i in range(len(actual)):
            if actual[i] != expected[i]:
                raise AssertionError("value differs")
    def assertGreater(self, actual: int, expected: int) -> None:
        assert actual > expected
"""
    if owner == "LineCacheTests":
        call = f"NativeCase().{method}()"
    else:
        expected = getattr(getattr(upstream, owner), "file_list", [])
        call = f"NativeCase(sys.argv[1], {expected!r}).{method}()"
    binary = lang.build(
        fixture.encode() + body + f"\n{call}\nprint('completed')\n".encode(), directory
    )

    def execute(case, original):
        file = upstream.FILENAME if owner == "LineCacheTests" else case.file_name
        proc = subprocess.run(
            [binary, file], capture_output=True, text=True, timeout=20
        )
        if (proc.returncode, proc.stdout, proc.stderr) != (0, "completed\n", ""):
            raise AssertionError((proc.returncode, proc.stdout, proc.stderr))
        return {
            "roles": roles(
                body="imre", assertions="imre", operation="imre", fixture="mixed"
            ),
            "evidence": {
                "operation": "native linecache file decoding or frozen-file cache; native assertion compares each element",
                "method_sha256": hashlib.sha256(body).hexdigest(),
                "opt_level": lang.level,
                "fixture": "Real CPython TestCase lifecycle supplies its original on-disk filename; native typed instance and assertion adapter.",
            },
        }

    return execute


def aliases(lang, root):
    directory = root / "aliases"
    directory.mkdir()
    src = HERE / "controls.py"
    exe = lang.build(src.read_bytes(), directory)
    file, absent = root / "source.py", root / "absent.py"
    outputs = []
    for invocation in ([sys.executable, src], [exe]):
        file.write_bytes("# cöldé\r\nsecond".encode())
        proc = subprocess.run(
            [*invocation, file, absent], cwd=root, capture_output=True, check=True
        )
        outputs.append((proc.stdout, proc.stderr))
    assert outputs[0] == outputs[1] and not outputs[0][1], outputs


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for arg in ("imre", "runtime", "clang", "result"):
        p.add_argument("--" + arg, type=Path, required=True)
    p.add_argument("--cpython-source", type=Path)
    a = p.parse_args()
    if sys.version_info[:3] != (3, 14, 5):
        p.error("use CPython 3.14.5 including its tests")
    if a.cpython_source:
        sys.path.insert(0, str(a.cpython_source.resolve() / "Lib"))
    from test import test_linecache as upstream

    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    reference = json.loads((HERE / "reference-linux.json").read_text())
    reports = []
    for level in (0, 3):
        lang = Toolchain(
            a.imre.resolve(), a.runtime.resolve(), a.clang.resolve(), level
        )
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            aliases(lang, root)
            check_search(lang, root)
            worker_dir = root / "worker"
            worker_dir.mkdir()
            binary = lang.build((HERE / "worker.py").read_bytes(), worker_dir)
            cases = {}
            for owner, method in (
                [(o, m) for o in BAD for m in ("test_getline", "test_getlines")]
                + [(o, "test_getlines") for o in GOOD]
                + [("LineCacheTests", "test_frozen")]
            ):
                where = root / f"{owner}-{method}"
                where.mkdir()
                cases[f"{MODULE}.{owner}.{method}"] = native_method(
                    lang, where, upstream, owner, method
                )
            bridge = Calls(binary)
            try:
                total = check_files(bridge, root)
                print(
                    f"-O{level}: {total} source-byte fixtures; cache timestamp, codec and deletion checks passed",
                    flush=True,
                )
                for owner, methods in HOST.items():
                    for method in methods:
                        cases[f"{MODULE}.{owner}.{method}"] = host_method(
                            bridge, owner, method, level
                        )
                with patch.object(upstream, "linecache", bridge):
                    report = run_evidence(load_suite(MODULE), cases, reference, level)
            finally:
                bridge.close()
            for row in report["records"]:
                if row["id"] in cases and row["outcome"] != "pass":
                    raise AssertionError(row)
            reports.append(report)
            print(f"-O{level}: {report['counts']}", flush=True)
    write_reports(a.result, reports, reference)


if __name__ == "__main__":
    main()
