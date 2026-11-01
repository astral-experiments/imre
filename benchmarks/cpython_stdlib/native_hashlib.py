"""Run pinned MD5 and SHA vectors and hash operations against native Imre."""
import argparse
import ast
from collections import Counter
from copy import copy
import hashlib
import inspect
from itertools import count
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from upstream import verify_upstream

VECTOR_COUNTS = {"md5": 3, "sha1": 4, "sha224": 4, "sha256": 4, "sha384": 4, "sha512": 4}
SHA3 = ("sha3_224", "sha3_256", "sha3_384", "sha3_512")
SHAKE = ("shake_128", "shake_256")
ALGORITHMS = (*VECTOR_COUNTS, *SHA3, *SHAKE)

DRIVER = """import hashlib
import sys

has_data = bool(sys.argv[1])
data = b""
if has_data:
    with open(sys.argv[1], "rb") as stream:
        data = stream.read()
algorithm = sys.argv[2]
secure = sys.argv[4] == "True"
if sys.argv[3] == "new":
    first = (hashlib.new(algorithm, data, usedforsecurity=secure) if has_data else
             hashlib.new(algorithm, usedforsecurity=secure))
""" + "".join(f'''elif algorithm == "{algorithm}":
    first = (hashlib.{algorithm}(data, usedforsecurity=secure) if has_data else
             hashlib.{algorithm}(usedforsecurity=secure))
''' for algorithm in ALGORITHMS) + """else:
    raise ValueError("unknown hash algorithm")
states = [first]
for index in range(5, len(sys.argv), 3):
    operation = sys.argv[index]
    state = states[int(sys.argv[index + 1])]
    if operation == "copy":
        states.append(state.copy())
    elif operation == "update":
        with open(sys.argv[index + 2], "rb") as stream:
            state.update(stream.read())
    elif operation == "name":
        print(repr(state.name))
    elif operation == "repr":
        print(repr(repr(state)))
    elif operation == "block_size":
        print(repr(state.block_size))
    elif operation == "digest_size":
        print(repr(state.digest_size))
    elif operation == "hexdigest":
        if sys.argv[index + 2]:
            print(repr(state.hexdigest(int(sys.argv[index + 2]))))
        else:
            print(repr(state.hexdigest()))
    elif operation == "digest":
        if sys.argv[index + 2]:
            print(repr(state.digest(int(sys.argv[index + 2]))))
        else:
            print(repr(state.digest()))
    else:
        raise ValueError("unknown hash operation")
"""


NAMES = r"""import hashlib
from hashlib import algorithms_available
print("sha1" in algorithms_available)
print("sha256" in algorithms_available)
algorithms_available.add("imre-control-unknown")
print("imre-control-unknown" in hashlib.algorithms_available)
for name in ["sha1", "SHA-1", "ssl3-sha1", "1.3.14.3.2.26", "sha_1", "sha1\\x00", "sha256", "Sha256", "SHA-256", "sha2-256", "2.16.840.1.101.3.4.2.1", "", "spam", "sha256\x00", "imre-control-unknown"]:
    try:
        state = hashlib.new(name=name, data=b"abc", usedforsecurity=False)
        copied = state.copy()
        state.update(b"d")
        print(repr((state.name, state.digest_size, state.block_size, copied.hexdigest(), state.hexdigest())))
    except ValueError as error:
        print(repr(str(error)))
"""


def payload(directory, data):
    with tempfile.NamedTemporaryFile(dir=directory, delete=False) as path:
        path.write(data)
    return path.name


MISSING = object()


class NativeHash:
    """Replay the calls on one native hash and its copies, preserving their shared history."""

    def __init__(self, program, directory, algorithm, variant, counts, data=MISSING, *, usedforsecurity=True):
        self.directory = directory
        source = "" if data is MISSING else payload(directory, data)
        self.command = [program, source, algorithm, variant, str(usedforsecurity)]
        self.slot = 0
        self.slots = count(1)
        self.counts = counts
        self.execute()
        counts["construct"] += 1

    def execute(self):
        result = subprocess.run(self.command, capture_output=True, timeout=30)
        if result.returncode or result.stderr:
            raise AssertionError(f"native hash driver exited {result.returncode}: {result.stderr!r}")
        lines = result.stdout.splitlines()
        reads = sum(op not in ("copy", "update") for op in self.command[5::3])
        if len(lines) != reads:
            raise AssertionError(f"native hash driver returned {len(lines)} results for {reads} reads")
        return lines

    def operation(self, name, argument=""):
        self.command.extend((name, str(self.slot), argument))
        lines = self.execute()
        key = f"{name}({argument})" if name in ("digest", "hexdigest") and argument else name
        self.counts[key] += 1
        if name not in ("copy", "update"):
            return ast.literal_eval(lines[-1].decode("ascii"))

    def update(self, data):
        self.operation("update", payload(self.directory, data))

    def copy(self):
        self.operation("copy")
        other = copy(self)
        other.slot = next(self.slots)
        return other

    def __repr__(self):
        return self.operation("repr")

    def hexdigest(self, length=MISSING):
        return self.operation("hexdigest", "" if length is MISSING else str(length))

    def digest(self, length=MISSING):
        return self.operation("digest", "" if length is MISSING else str(length))

    @property
    def name(self):
        return self.operation("name")

    @property
    def block_size(self):
        return self.operation("block_size")

    @property
    def digest_size(self):
        return self.operation("digest_size")


def run_case(suite, name, algorithms, program, work):
    case = suite.HashLibTestCase(name)
    counts = {algorithm: {"direct": Counter(), "new": Counter()} for algorithm in algorithms}
    host_calls = Counter()
    private = {}
    lengths = {"direct": [], "new": []}
    original_new = suite.hashlib.new
    vector = name.startswith("test_case_")
    public = {}

    def native(algorithm, variant, data=MISSING, *, usedforsecurity=True):
        if vector:
            lengths[variant].append(len(data))
        return NativeHash(program, work, algorithm, variant, counts[algorithm][variant], data,
                          usedforsecurity=usedforsecurity)

    def direct(algorithm, original):
        def construct(*args, **kwargs):
            if not args and "data" not in kwargs and vector:
                host_calls["empty direct constructor for file_digest"] += 1
                return original(**kwargs)
            return native(algorithm, "direct", *args, **kwargs)
        return construct

    def named(algorithm, *args, **kwargs):
        if algorithm not in counts:
            return original_new(algorithm, *args, **kwargs)
        if not args and "data" not in kwargs and vector:
            # Preserve the upstream crypto-policy probe and both file_digest fixtures.
            host_calls["empty named constructor for file_digest or policy probe"] += 1
            return original_new(algorithm, **kwargs)
        return native(algorithm, "new", *args, **kwargs)

    for algorithm in algorithms:
        constructors = case.constructors_to_test[algorithm]
        original = getattr(suite.hashlib, algorithm)
        constructors.remove(original)
        # Keep upstream's named-constructor closure: its actual hashlib.new lookup
        # reaches the named native path. The remaining private providers run unchanged.
        for ctor in constructors.copy():
            if ctor.__module__ != suite.__name__:
                spy = mock.Mock(wraps=ctor)
                private[f"{ctor.__module__}.{ctor.__name__}"] = spy
                constructors.remove(ctor)
                constructors.add(spy)
        public[algorithm] = direct(algorithm, original)
        constructors.add(public[algorithm])

    with (
        mock.patch.multiple(suite.hashlib, new=named, **public),
        mock.patch.object(case, "check", wraps=case.check) as checks,
        mock.patch.object(suite.hashlib, "file_digest", wraps=suite.hashlib.file_digest) as files,
    ):
        result = unittest.TestResult()
        case.run(result)
    host_calls["file_digest"] = files.call_count
    diagnostics = [reason for _, reason in result.failures + result.errors]
    if result.wasSuccessful() and not result.skipped:
        for algorithm, variants in counts.items():
            for variant, observed in variants.items():
                if vector:
                    # A cached URL resource can supply multiple vectors in one case.
                    n = max(1, checks.call_count)
                    expected = {"construct": n, "hexdigest": n, "digest": n}
                    if algorithm not in SHAKE:
                        expected["digest_size"] = n
                elif name == "test_large_update":
                    expected = {"construct": 4, "update": 6, "name": 1, "digest": 8, "copy": 1}
                elif name == "test_hexdigest":
                    expected = {"construct": 1, "name": 1, "digest": 2, "hexdigest": 1}
                elif name in ("test_blocksize_and_name", "test_blocksize_name_sha3"):
                    expected = {"construct": 1, "block_size": 1, "digest_size": 1,
                                "digest": 1, "hexdigest": 1, "name": 1, "repr": 1}
                elif name == "test_name_attribute":
                    # Every provider, including private CPython constructors, also
                    # constructs a hash by name to compare its canonical name.
                    extra = len(case.constructors_to_test[algorithm]) if variant == "new" else 0
                    expected = {"construct": 1 + extra, "name": 5 + extra}
                elif name in ("test_usedforsecurity_true", "test_usedforsecurity_false"):
                    # The method explicitly probes named SHA-256 and public MD5 in
                    # addition to the two calls on each registered constructor.
                    extra = algorithm == "md5" or (algorithm == "sha256" and variant == "new")
                    expected = {"construct": 2 + extra}
                else:
                    raise ValueError(f"unknown case {name}")
                totals = Counter()
                for operation, count in observed.items():
                    totals[operation.partition("(")[0]] += count
                if totals != expected:
                    diagnostics.append(f"{algorithm} {variant}: expected {expected}, observed {dict(observed)}")
    passed = result.wasSuccessful() and result.testsRun == 1 and not diagnostics
    row = {
        "status": "mixed" if passed else "fail",
        "native_operations": counts,
        "host_operations": dict(host_calls),
        "host_reference_constructors": {key: spy.call_count for key, spy in sorted(private.items())},
        "diagnostics": diagnostics,
    }
    if passed and result.skipped:
        row["status"] = "skip"
        row["skip_reasons"] = [reason for _, reason in result.skipped]
    if vector:
        row["native_one_shot_input_bytes"] = lengths
    elif name in ("test_blocksize_and_name", "test_blocksize_name_sha3"):
        row["unmodified_cpython_algorithms"] = []
    else:
        row["unmodified_cpython_algorithms"] = sorted(case.constructors_to_test.keys() - counts.keys())
    return case.id(), row


def werkzeug_etags(wheelhouse, work, build):
    # The original module runs on CPython; compile the exact wheel function
    # separately so unrelated imports do not hide its executable behavior.
    sys.path.insert(0, str(HERE.parent / "flask"))
    from gate import stage
    site = work / "site"
    site.mkdir()
    stage(wheelhouse, site)
    sys.path.insert(0, str(site))
    from werkzeug.http import generate_etag

    source = work / "etag.py"
    body = inspect.getsource(generate_etag)
    source.write_text("from hashlib import sha1\nimport sys\n\n" + body +
                      '\nwith open(sys.argv[1], "rb") as f:\n    print(generate_etag(f.read()))\n')
    program = build(source)
    payload = work / "etag-data"
    lengths = []
    for data in (b"", b"hello", bytes(range(256)), b"x" * 55, b"x" * 56, b"x" * 64, b"a" * 1_000_000):
        payload.write_bytes(data)
        actual = subprocess.check_output([program, payload], timeout=30).decode().strip()
        assert actual == generate_etag(data), (len(data), actual)
        lengths.append(len(data))
    return {"function_sha256": hashlib.sha256(body.encode()).hexdigest(), "matched_input_bytes": lengths}


def run_native_methods(suite, work, build):
    methods = {"test_sha256_gil": 2, "test_sha256_update_over_4gb": 1, "test_new_upper_to_lower": 1}
    source = Path(suite.__file__).read_bytes()
    cls = next(n for n in ast.parse(source).body if isinstance(n, ast.ClassDef) and n.name == "HashLibTestCase")
    lines = source.splitlines(keepends=True)
    bodies = {n.name: b"".join(lines[n.lineno - 1:n.end_lineno]) for n in cls.body
              if isinstance(n, ast.FunctionDef) and n.name in methods}
    # unittest opts the large test out without the CPU resource. This benchmark
    # opts it in and copies the original method without its scheduling decorator.
    # The private CPython module constants in the other fixture are host input.
    threshold = suite.hashlib_helper.find_gil_minsize(["_sha2", "_hashlib"])
    (work / "hashlib_helper.py").write_text(f"""def find_gil_minsize(modules: list[str]) -> int:
    if len(modules) != 2 or modules[0] != "_sha2" or modules[1] != "_hashlib":
        raise ValueError("unexpected SHA-256 threshold fixture")
    return {threshold}
""")
    adapter = b"""import hashlib
import hashlib_helper
import sys

class Case:
    checks: int

    def __init__(self) -> None:
        self.checks = 0

    def assertEqual(self, actual: str, expected: str) -> None:
        if actual != expected:
            raise ValueError(f"expected {expected}, got {actual}")
        self.checks += 1

"""
    dispatch = "case = Case()\n" + "".join(f'if sys.argv[1] == "{name}":\n    case.{name}()\n' for name in methods)
    driver = work / "methods.py"
    driver.write_bytes(adapter + b"\n".join(bodies[name] for name in methods) + b"\n" +
                       (dispatch + "print(case.checks)\n").encode())
    binary = build(driver)
    tests = {}
    for name, checks in methods.items():
        run = subprocess.run([binary, name], cwd=work, text=True, capture_output=True, timeout=120)
        passed = not run.returncode and not run.stderr and run.stdout == f"{checks}\n"
        row = {"status": "pass" if passed else "fail", "native_assertions": checks if passed else None,
               "method_sha256": hashlib.sha256(bodies[name]).hexdigest()}
        if name == "test_sha256_gil":
            row["host_fixture_gil_minsize"] = threshold
        if not passed:
            row["diagnostics"] = [run.stderr or repr(run.stdout)]
        tests[suite.HashLibTestCase(name).id()] = row
    return tests


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--cpython-source", type=Path)
    parser.add_argument("--reachable-only", action="store_true")
    parser.add_argument("--wheelhouse", type=Path)
    parser.add_argument("--opt-level", choices=("0", "3"), default="3")
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("the original hashlib fixtures require CPython 3.14.5")
    commit = verify_upstream()
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    import test
    vendored = HERE / "upstream/Lib/test"
    test.__path__.insert(0, str(vendored))
    from test import test_hashlib
    if Path(test_hashlib.__file__).resolve() != vendored / "test_hashlib.py":
        raise RuntimeError("the selected methods must come from the verified vendored file")

    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        def build(source):
            program = source.with_suffix("")
            subprocess.run([args.imre.resolve(), "build", source, "-o", program, "--no-cache",
                            "--opt-level", args.opt_level, "--clang", args.clang.resolve(), "--runtime", args.runtime.resolve(),
                            *(["--reachable-only"] if args.reachable_only else [])],
                           cwd=work, check=True, timeout=120)
            return program

        source = work / "hash.py"
        source.write_text(DRIVER)
        program = build(source)
        cases = dict(run_case(test_hashlib, f"test_case_{alg}_{n}", [alg], program, work)
                     for alg, size in VECTOR_COUNTS.items() for n in range(size))
        # A standalone interpreter enables all test resources. Use the default
        # regrtest configuration for the original URL-vector methods.
        with mock.patch.object(test_hashlib.support, "use_resources", {}):
            for alg in (*SHA3, *SHAKE):
                url_name = alg.replace("shake_", "shake")
                cases.update(run_case(test_hashlib, name, [alg], program, work)
                             for name in (f"test_case_{alg}_0", f"test_case_{url_name}_vector"))
        cases.update(run_case(test_hashlib, name, ["md5"], program, work)
                     for name in ("test_case_md5_huge", "test_case_md5_uintmax"))
        cases.update(run_case(test_hashlib, name, ALGORITHMS, program, work)
                     for name in ("test_large_update", "test_hexdigest", "test_name_attribute",
                                  "test_usedforsecurity_true", "test_usedforsecurity_false"))
        cases.update([run_case(test_hashlib, "test_blocksize_and_name", VECTOR_COUNTS, program, work),
                      run_case(test_hashlib, "test_blocksize_name_sha3", (*SHA3, *SHAKE), program, work)])
        etags = werkzeug_etags(args.wheelhouse, work, build) if args.wheelhouse else None
        controls = work / "names.py"
        controls.write_text(NAMES)
        binary = build(controls)
        def run(command):
            return subprocess.run(command, capture_output=True, timeout=30)
        reference, actual = run([sys.executable, controls]), run([binary])
        assert reference.returncode == 0 and not reference.stderr, reference
        assert (actual.returncode, actual.stdout, actual.stderr) == (0, reference.stdout, b""), actual
        native = run_native_methods(test_hashlib, work, build)
        cases.update(native)
    report = {"cpython_commit": commit, "named_provider_controls": "matched CPython", "opt_level": args.opt_level, "whole_native_methods": sum(r["status"] == "pass" for r in native.values()),
              "tests": cases}
    report["reachable_only"] = args.reachable_only
    if etags is not None:
        report["werkzeug_generate_etag"] = etags
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    for case, row in cases.items():
        print(row["status"], case)
        for diagnostic in row.get("diagnostics", ()):
            print(diagnostic, file=sys.stderr)
    return int(any(row["status"] == "fail" for row in cases.values()))


if __name__ == "__main__":
    sys.exit(main())
