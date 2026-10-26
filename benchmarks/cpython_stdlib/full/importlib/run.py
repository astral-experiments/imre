"""Run original importlib spec cases against a native heap, preserving the full suite denominator."""

import argparse
import ast
import hashlib
import json
import select
import subprocess
import sys
import tempfile
from collections import Counter
from contextlib import contextmanager
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parents[1])]
from cache_evidence import register
from product_controls import check
from suite_evidence import reference_cases, roles, write_reports
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, method_bytes, run_evidence

SOURCE = HERE / "upstream/Lib/test/test_importlib/test_spec.py"
METHODS = (
    "test_default",
    "test_default_no_loader",
    "test_default_is_package_false",
    "test_default_is_package_true",
    "test_has_location_setter",
    "test_package",
    "test_package_is_package",
    "test_cached_set",
    "test_cached_no_origin",
    "test_cached_with_origin_not_location",
    "test_cached_source_unknown_suffix",
    "test_cached_source",
    "test_cached_source_missing_cache_tag",
    "test_cached_sourceless",
)


class Spec:
    def __init__(self, worker, slot):
        self.__dict__.update(_worker=worker, _slot=slot)

    def __getattr__(self, name):
        result = self._worker.call(name, self._slot)
        if name == "loader" and result is not None:
            assert result == 1
            return self._worker.loader
        if name == "loader_state":
            assert result is True  # the worker performed the None identity check
            return None
        return result

    def __setattr__(self, name, value):
        if name in {"cached", "origin"}:
            fields = self._worker.text(value)
        elif name == "has_location" and isinstance(value, bool):
            fields = (str(value),)
        else:
            raise AssertionError((name, value))
        self._worker.call("set_" + name, self._slot, *fields)


class Heap:
    """Opaque loader handle 1 is resolved only after checking native object identity."""

    def __init__(self, executable, loader):
        self.proc = subprocess.Popen(
            [executable],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        self.loader = loader
        self.calls = Counter()

    def call(self, op, *fields):
        self.calls[op] += 1
        if op == "cached":
            fields = (*fields, *self.text(sys.implementation.cache_tag), *self.text(sys.pycache_prefix))
        request = [op, *map(str, fields)]
        assert not any("\n" in x for x in request)
        self.proc.stdin.write("\n".join(request) + "\n")
        self.proc.stdin.flush()
        if not select.select([self.proc.stdout], [], [], 20)[0]:
            raise TimeoutError(op)
        row = self.proc.stdout.readline()
        if not row:
            raise AssertionError((op, self.proc.stderr.read()))
        return ast.literal_eval(row)

    @staticmethod
    def text(value):
        return ("None",) if value is None else ("str", value)

    def ModuleSpec(self, name, loader, *, origin=None, is_package=None):
        assert loader is self.loader or loader is None
        package = "default" if is_package is None else str(is_package)
        index = self.call(
            "new",
            name,
            "none" if loader is None else "loader",
            *self.text(origin),
            package,
        )
        return Spec(self, index)

    def import_fixture(self, source):
        assert source._cached is None and source.loader_state is None
        spec = self.ModuleSpec(source.name, source.loader, origin=source.origin)
        spec.has_location = source._set_fileattr
        return spec

    def close(self):
        try:
            self.proc.stdin.write("quit\n")
            self.proc.stdin.flush()
            stdout, stderr = self.proc.communicate(timeout=10)
            if (self.proc.returncode, stdout, stderr) != (0, "", ""):
                raise AssertionError((self.proc.returncode, stdout, stderr))
        finally:
            if self.proc.poll() is None:
                self.proc.kill()
                self.proc.wait()


@contextmanager
def fixture(case, binary):
    machinery, spec, loc_spec = case.machinery, case.spec, case.loc_spec
    heap = Heap(binary, case.loader)
    try:
        case.machinery = heap
        case.spec = heap.import_fixture(spec)
        case.loc_spec = heap.import_fixture(loc_spec)
        yield heap
    finally:
        case.machinery, case.spec, case.loc_spec = machinery, spec, loc_spec
        heap.close()


def host_method(binary, method, level):
    body = method_bytes(SOURCE, "ModuleSpecTests", method)

    def execute(case, original):
        with fixture(case, binary) as heap:
            before = heap.calls.copy()
            original()
            calls = heap.calls - before
        if not calls or set(calls) == {"new"}:
            raise AssertionError((case.id(), calls))
        return {
            "roles": roles(
                body="cpython", assertions="cpython", operation="imre", fixture="mixed"
            ),
            "evidence": {
                "operation": "All observed spec construction, attribute access and mutation use one live native heap.",
                "fixture": "Original CPython Frozen/Source setUp and TestCase assertions; native heap reconstructs both initial specs and retains the fixture loader by identity handle. The cached getter sees the live fixture tag/prefix. For test_cached_source the original CPython util independently computes its expected value; the observed ModuleSpec getter is native.",
                "native_calls": dict(sorted(calls.items())),
                "opt_level": level,
                "method_sha256": hashlib.sha256(body).hexdigest(),
            },
        }

    return execute


NATIVE = {
    "test_package",
    "test_package_is_package",
    "test_has_location_setter",
    "test_cached_no_origin",
    "test_cached_with_origin_not_location",
}


def native_case(lang, root, method):
    body = method_bytes(SOURCE, "ModuleSpecTests", method)
    fixture_source = b"import sys\n" + (HERE / "native_fixture.py").read_bytes()
    call = f"\nNativeCase(sys.argv[1], sys.argv[2]).{method}()\nprint('completed')\n".encode()
    root.mkdir()
    binary = lang.build(fixture_source + b"\n" + body + call, root)

    def execute(case, original):
        result = subprocess.run(
            [binary, case.name, case.path],
            check=False,
            capture_output=True,
            text=True,
            timeout=20,
        )
        if (result.returncode, result.stdout, result.stderr) != (0, "completed\n", ""):
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        return {
            "roles": roles(
                body="imre", assertions="imre", operation="imre", fixture="mixed"
            ),
            "evidence": {
                "operation": "Complete byte-identical original method uses native ModuleSpec and strict typed equality, None-identity or boolean assertions.",
                "fixture": "Real Frozen/Source CPython setUp supplies name and path. Native typed fixture supplies an opaque loader; these methods never invoke a loader callback.",
                "method_sha256": hashlib.sha256(body).hexdigest(),
                "opt_level": lang.level,
            },
        }

    return execute


# These are the actual conditions on the pinned decorators, not native exclusions.
def skips(suite, ref, level):
    from collections import Counter

    seen = Counter()
    conditions = {
        "requires a case-insensitive filesystem": (
            "filesystem is case-sensitive in test_util.case_insensitive_tests",
            "util.py:123",
        ),
        "pyfakefs not available": (
            "pyfakefs import failed in FakeFileSystem",
            "metadata/stubs.py:10",
        ),
        "Desired but not supported.": (
            "unconditional upstream unittest.skip on zip resources",
            "resources/test_resource.py:151",
        ),
        "test meaningful only where os.altsep is defined": (
            "os.sep != '\\' or os.altsep != '/'",
            "test_util.py:450",
        ),
        "No module named 'winreg'": (
            "winreg unavailable on Linux",
            "test_windows.py:13",
        ),
    }
    bigmem = {
        "test_parallel_module_init": 138,
        "test_parallel_meta_path": 142,
        "test_parallel_path_hooks": 153,
        "test_concurrent_futures_circular_import": 248,
        "test_multiprocessing_pool_circular_import": 255,
    }
    result = {}
    refs = reference_cases(ref)
    for case in cases(suite):
        name = case.id()
        seen[name] += 1
        key = name, seen[name]
        ref_result = refs[key]
        if not isinstance(ref_result, dict):
            continue
        reason = ref_result["skipped"]
        if reason.startswith("not enough memory:"):
            method = name.rsplit(".", 1)[1]
            condition, source = (
                "bigmemtest dry_run=False and no requested memory limit",
                f"test_threaded_import.py:{bigmem[method]}",
            )
        else:
            condition, source = conditions[reason]
        cls = type(case)
        result[(*key, level)] = {
            "kind": "upstream-skip",
            "fixture_variant": f"{cls.__module__}.{cls.__qualname__}",
            "platform": "linux",
            "configuration": "CPython 3.14.5 Linux, bytecode enabled, default regrtest resources",
            "condition": condition,
            "reason": reason,
            "source": f"Lib/test/test_importlib/{source}",
        }
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for option in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + option, type=Path, required=True)
    parser.add_argument("--cpython-source", type=Path)
    args = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5):
        parser.error("use test-enabled CPython 3.14.5")
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    manifest = json.loads((HERE / "UPSTREAM.json").read_bytes())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    ref = json.loads((HERE / "reference-linux.json").read_bytes())
    reports = []
    for level in (0, 3):
        lang = Toolchain(args.imre, args.runtime, args.clang, level)
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            check(lang, root)
            binary = lang.build((HERE / "worker.py").read_bytes(), root)
            executions = {
                f"test.test_importlib.test_spec.{variant}_ModuleSpecTests.{method}": host_method(
                    binary, method, level
                )
                for variant in ("Frozen", "Source")
                for method in METHODS
            }
            for method in sorted(NATIVE):
                execute = native_case(lang, root / method, method)
                for variant in ("Frozen", "Source"):
                    executions[
                        f"test.test_importlib.test_spec.{variant}_ModuleSpecTests.{method}"
                    ] = execute
            register(lang, root, level, executions)
            suite = load_suite(manifest["module"])
            report = run_evidence(
                suite, executions, ref, level, exceptions=skips(suite, ref, level)
            )
            for row in report["records"]:
                if row["id"] in executions and row["outcome"] != "pass":
                    raise AssertionError(row)
            reports.append(report)
            print(f"-O{level}: {report['counts']}")
    write_reports(args.result, reports, ref)


if __name__ == "__main__":
    main()
