"""Run pinned cache methods with a persistent native cache for each decorated function."""
import argparse
import ast
import builtins
from collections import Counter
import inspect
import json
from pathlib import Path
import random
import select
import subprocess
import sys
import tempfile
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests
from suite_native import run_in_fixtures

# Transport limitations raise a distinct exception: they must never satisfy an
# upstream assertRaises(TypeError) for a missing product operation.
class UnsupportedFixture(Exception):
    pass


length = len


class Snapshot(tuple):
    """Transport positional values and a handle to this exact snapshot in the native child."""

    def __new__(cls, values, wrapper, token):
        result = super().__new__(cls, values)
        result.wrapper = wrapper
        result.token = token
        return result

    def __getattr__(self, name):
        if name not in ("hits", "misses", "maxsize", "currsize"):
            raise AttributeError(name)
        return self.wrapper.read_field(self.token, name)


class Build:
    def __init__(self, directory, args, level):
        self.root, self.args, self.level = directory, args, level
        self.binaries = {}
        self.extension = directory / "extension"
        self.extension.mkdir()
        header = subprocess.check_output([args.imre, "extension-header"])
        (self.extension / "Python.h").write_bytes(header)
        (self.extension / "cache_fixture.pyi").write_bytes((HERE / "cache_fixture.pyi").read_bytes())
        obj = directory / "fixture.o"
        subprocess.run([args.clang, "-std=c11", "-fPIC", f"-O{level}", "-I", self.extension, "-c",
                        HERE / "cache_fixture.c", "-o", obj], check=True)
        subprocess.run(["ar", "rcs", self.extension / "cache_fixture.imre.a", obj], check=True)

    def binary(self, decorator, shape):
        key = decorator, shape
        if key not in self.binaries:
            directory = self.root / f"driver-{length(self.binaries)}"
            directory.mkdir()
            source = (HERE / "cache_driver.py.in").read_text()
            source = source.replace("DECORATOR", decorator)
            source = source.replace("PARAMETERS", ", ".join(f"a{i}: {t.__name__}" for i, t in enumerate(shape)))
            source = source.replace("ARGUMENTS", ", ".join("int(input())" if t is int else "input()" for t in shape))
            entry, binary = directory / "driver.py", directory / "driver"
            entry.write_text(source)
            subprocess.run([self.args.imre, "build", entry, "-o", binary, "--opt-level", str(self.level),
                            "--no-cache", "--runtime", self.args.runtime, "--clang", self.args.clang,
                            "--extension-path", self.extension], check=True, timeout=120)
            self.binaries[key] = binary
        return self.binaries[key]


class Wrapper:
    def __init__(self, build, function, decorator, counts):
        self.build, self.function, self.decorator, self.counts = build, function, decorator, counts
        self.proc = self.shape = self.failure = None
        self.objects = []
        self.arity = length(inspect.signature(function).parameters)
        self.bound_len_calls = 0

    def start(self, shape):
        if any(t not in (int, str) for t in shape):
            raise UnsupportedFixture(f"native cache fixture needs exact integer or string keys: {shape!r}")
        if self.proc is None:
            self.shape = shape
            self.proc = subprocess.Popen([self.build.binary(self.decorator, shape)],
                                         stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                         text=True)
        elif self.shape != shape:
            raise UnsupportedFixture(f"cache wrapper has one native argument layout: {self.shape!r} vs {shape!r}")

    def send(self, *values):
        if self.failure is not None:
            raise self.failure
        try:
            self.proc.stdin.write("".join(f"{value}\n" for value in values))
            self.proc.stdin.flush()
        except BrokenPipeError:
            raise self.closed() from None

    def closed(self):
        # EOF and EPIPE come from the same child. Preserve its first diagnostic
        # when a reentrant callback unwinds or the fixture closes the driver.
        if self.failure is None:
            out, err = self.proc.communicate(timeout=5)
            self.failure = RuntimeError(
                f"native cache fixture exit {self.proc.returncode}: {err.strip() or out.strip() or 'unexpected EOF'}")
        return self.failure

    def read(self):
        if not select.select([self.proc.stdout], [], [], 15)[0]:
            raise RuntimeError("native cache fixture timed out")
        line = self.proc.stdout.readline().rstrip("\n")
        if not line:
            raise self.closed()
        return line

    def __call__(self, *args, **kwargs):
        if self.function is length and builtins.len is self:
            self.bound_len_calls += 1
            self.counts["host_len_binding_calls"] += 1
        if kwargs:
            raise UnsupportedFixture("native cached calls do not yet retain keyword order and names")
        if self.arity != length(args):
            raise UnsupportedFixture("native cached calls require a fixed positional argument list")
        if any(type(a) is str and ("\n" in a or "\r" in a) for a in args):
            raise UnsupportedFixture("string key is not representable in the line protocol")
        self.start(tuple(map(type, args)))
        self.send(0, *args)
        self.counts["lookups"] += 1
        original = None
        reply = self.read()
        if reply == "body":
            self.counts["callback_calls"] += 1
            try:
                result = self.function(*args)
            except Exception as exc:
                original = exc
                self.send(-2)
            else:
                token = length(self.objects)
                self.objects.append(result)
                self.send(-1, token)
            reply = self.read()
        if reply == "raised" and original is not None:
            self.counts["host_exception_state"] += 1
            raise original
        if original is not None or not reply.startswith("result "):
            raise RuntimeError(f"unexpected native cache response: {reply}")
        return self.objects[int(reply.split()[1])]

    def save_snapshot(self):
        self.start(self.shape if self.shape is not None else (int,) * self.arity)
        self.send(1)
        op, token, hits, misses, size, current = self.read().split()
        if op != "info":
            raise RuntimeError(op)
        values = (int(hits), int(misses), None if size == "None" else int(size), int(current))
        return int(token), values

    def cache_info(self):
        token, values = self.save_snapshot()
        self.counts["snapshots"] += 1
        return Snapshot(values, self, token)

    def read_field(self, token, name):
        self.send(4, token, name)
        tag, value = self.read().split()
        if tag != "field":
            raise RuntimeError(f"unexpected cache_info field response: {tag}")
        self.counts["native_named_fields"] += 1
        return None if value == "None" else int(value)

    def expected_info(self, values):
        self.start(self.shape if self.shape is not None else (int,) * self.arity)
        self.send(5, *values)
        tag, _, payload = self.read().partition(" ")
        if tag != "expected":
            raise RuntimeError(f"unexpected _CacheInfo constructor response: {tag}")
        self.counts["native_expected_constructors"] += 1
        return ast.literal_eval(payload)

    def cache_clear(self):
        self.start(self.shape if self.shape is not None else (int,) * self.arity)
        self.send(2)
        if self.read() != "cleared":
            raise RuntimeError("cache_clear did not finish")
        self.counts["clears"] += 1

    def cache_parameters(self):
        self.start(self.shape if self.shape is not None else (int,) * self.arity)
        self.send(3)
        tag, _, payload = self.read().partition(" ")
        if tag != "parameters":
            raise RuntimeError(f"unexpected cache_parameters response: {tag}")
        self.counts["parameter_reads"] += 1
        return ast.literal_eval(payload)

    @property
    def __wrapped__(self):
        self.counts["host_wrapper_metadata"] += 1
        return self.function

    def close(self):
        if self.proc is not None:
            proc = self.proc
            try:
                if self.failure is not None:
                    return
                out, err = proc.communicate(input="-3\n", timeout=5)
                if out or err or proc.returncode:
                    raise RuntimeError(f"native cache fixture exit {proc.returncode}: {out!r} {err!r}")
            finally:
                if proc.poll() is None:
                    proc.kill()
                    proc.wait()


# These methods use wrappers created at upstream module/class import, shared
# threads, or the C recursion limit. A proxy's host identity or thread behavior
# would give false credit, so retain an explicit blocker for each such ID.
BLOCKERS = {
    "test_pickle": "pickle of module/class-level cached functions requires native global callable identity",
    "test_copy": "copy of pre-existing cache wrappers and a partial requires native callable identity",
    "test_deepcopy": "deepcopy of pre-existing cache wrappers and a partial requires native callable identity",
    "test_lru_cache_weakrefable": "weak references to function/method wrappers would target the host proxy",
    "test_get_annotations": "annotationlib reads native wrapper metadata; host proxy metadata is not evidence",
    "test_get_annotations_with_forwardref": "deferred annotation callbacks and annotationlib.ForwardRef are not native",
    "test_lru_cache_decoration": "assigned wrapper metadata requires native update_wrapper",
    "test_lru_cache_threaded": "native cache state is thread-local; suite requires one cache shared across threads",
    "test_lru_cache_threaded2": "simultaneous misses in one shared cache require native cross-thread state",
    "test_lru_cache_threaded3": "simultaneous misses and eviction require native cross-thread state",
    "test_lru_method": "native method descriptors and int subclass cache keys are not yet supported",
}


def exercise(build, case, original, counts):
    wrappers = []
    initial_len = builtins.len
    bindings = []
    def decorate(function, decorator):
        value = Wrapper(build, function, decorator, counts)
        wrappers.append(value)
        if function is length:
            value.start((str,))  # test_lru_reentrancy_with_len replaces builtins.len after decoration
        return value

    def lru_cache(maxsize=128, typed=False):
        if callable(maxsize):
            return decorate(maxsize, "functools.lru_cache")
        if maxsize is not None and type(maxsize) is not int or type(typed) is not bool:
            raise UnsupportedFixture("cache fixture requires literal maxsize and typed")
        return lambda f: decorate(f, f"functools.lru_cache(maxsize={maxsize!r}, typed={typed!r})")

    def expected_info(*args, **kwargs):
        # The upstream expected tuples use these four integer/None keywords in this order.
        if (args or not wrappers or tuple(kwargs) != ("hits", "misses", "maxsize", "currsize")
                or any(type(v) is not int for k, v in kwargs.items() if k != "maxsize")
                or (kwargs["maxsize"] is not None and type(kwargs["maxsize"]) is not int)):
            raise UnsupportedFixture("unsupported _CacheInfo arguments in expected value")
        return wrappers[-1].expected_info(kwargs.values())

    # The host retains each distinct upstream module and runs its original method.
    try:
        with patch.object(case.module, "lru_cache", lru_cache), patch.object(
                case.module, "cache", lambda f: decorate(f, "functools.cache")), patch.object(
                case.module, "_CacheInfo", expected_info):
            original()
        for wrapper in wrappers:
            if wrapper.bound_len_calls:
                if builtins.len is not initial_len:
                    raise AssertionError("the original method did not restore builtins.len by identity")
                bindings.append({"binding": "builtins.len", "calls_while_replaced": wrapper.bound_len_calls,
                                 "cache_info_before_teardown": wrapper.save_snapshot()[1],
                                 "original_is_callback": wrapper.function is initial_len,
                                 "original_identity_restored": True})
    finally:
        for wrapper in reversed(wrappers):
            wrapper.close()
    if not any(counts[k] for k in ("lookups", "snapshots", "clears", "parameter_reads")):
        raise UnsupportedFixture("no native cache operation executed")
    mixed = any(value for name, value in counts.items() if name.startswith("host_"))
    result = {"status": "mixed" if mixed else "host_driven_native_operation", **counts}
    if bindings:
        result["binding_observations"] = bindings
    return result


def verify_snapshot_transport(build):
    """A handle must still read the old native tuple after a lookup, hit and clear."""
    counts = Counter()
    wrapper = Wrapper(build, lambda n: n, "functools.lru_cache(maxsize=2, typed=False)", counts)
    try:
        empty = wrapper.cache_info()
        wrapper(7)
        miss = wrapper.cache_info()
        wrapper(7)
        hit = wrapper.cache_info()
        wrapper.cache_clear()
        cleared = wrapper.cache_info()
        observed = [(snap.hits, snap.misses, snap.maxsize, snap.currsize)
                    for snap in (hit, empty, cleared, miss, hit)]
        if observed != [(1, 1, 2, 1), (0, 0, 2, 0), (0, 0, 2, 0), (0, 1, 2, 1), (1, 1, 2, 1)]:
            raise AssertionError(f"retained native snapshots changed: {observed}")
        return {"read_order": ["hit", "empty", "cleared", "miss", "hit"],
                "values": observed, "native_named_fields": counts["native_named_fields"]}
    finally:
        wrapper.close()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for arg in ("imre", "runtime", "clang", "result"):
        p.add_argument("--" + arg, type=Path, required=True)
    args = p.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        p.error("test setup needs CPython 3.14.5")
    for arg in ("imre", "runtime", "clang"):
        setattr(args, arg, getattr(args, arg).resolve())
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    ref = json.loads((HERE / "reference-linux.json").read_text())
    report = {"module": manifest["module"], "denominator": ref["denominator"],
              "cpython_commit": manifest["commit"], "optimization_levels": {}, "transport_controls": {}}
    failed = False
    for level in (0, 3):
        with tempfile.TemporaryDirectory() as temp:
            build = Build(Path(temp), args, level)
            report["transport_controls"][str(level)] = verify_snapshot_transport(build)
            all_cases = list(cases(load_suite(manifest["module"])))
            if {case.id() for case in all_cases} != ref["reference"].keys():
                raise RuntimeError("reference denominator changed")
            results, executions, counts = {}, {}, {}
            for case in all_cases:
                name = case.id()
                if name.split(".")[-2] not in {"TestCacheC", "TestCachePy", "TestLRUC", "TestLRUPy"}:
                    continue
                reason = BLOCKERS.get(case._testMethodName)
                if name.endswith("TestLRUC.test_lru_recursion"):
                    reason = "the C variant asserts a native recursion limit; host stack exhaustion is not evidence"
                if reason:
                    results[name] = {"status": "blocked", "diagnostic": reason}
                else:
                    counts[name] = Counter()
                    executions[name] = lambda case, method: exercise(build, case, method, counts[case.id()])
            # Both optimization levels receive the same stream of calls from random.choice.
            state = random.getstate()
            try:
                random.seed(0)
                evidence, failures, skips = run_in_fixtures(load_suite(manifest["module"]), executions)
            finally:
                random.setstate(state)
            if skips:
                raise RuntimeError(f"unexpected skips: {skips}")
            for name, entries in evidence.items():
                if entries:
                    entry, = entries
                    results[name] = entry
            for name, message in failures:
                diagnostic = message.strip().splitlines()[-1]
                blocked = "UnsupportedFixture:" in diagnostic
                results[name] = {"status": "blocked" if blocked else "failed", **counts[name],
                                 "diagnostic": diagnostic}
                failed |= not blocked
            if set(executions) - results.keys():
                raise RuntimeError("missing cache result")
            report["optimization_levels"][str(level)] = dict(sorted(results.items()))
            print(f"-O{level}: {dict(Counter(row['status'] for row in results.values()))}", flush=True)
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    return int(failed)


if __name__ == "__main__":
    sys.exit(main())
