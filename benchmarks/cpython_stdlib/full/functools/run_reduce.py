"""Run all seven original reduce routines with Imre performing the left fold."""
import argparse
from collections import Counter
import json
from operator import add
from pathlib import Path
import select
import subprocess
import sys
import tempfile
from unittest.mock import patch
import warnings

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, native_method, run_in_fixtures


def native_call(binary, mode, function, source, initial, counts):
    # Object identity and user-defined iterator/callable protocols belong to
    # the host fixture. The compiled reduce chooses when to request either.
    objects = [initial]
    iterator = None
    original = None
    counts["native_calls"] += 1
    with subprocess.Popen([binary, mode], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE, text=True) as proc:
        try:
            while True:
                if not select.select([proc.stdout], [], [], 15)[0]:
                    raise RuntimeError("native reduce timed out")
                line = proc.stdout.readline().rstrip("\n")
                if line.startswith(("result ", "type-error ")) or line == "remote exception":
                    break
                if not line:
                    raise RuntimeError(f"native reduce closed the protocol: {proc.stderr.read()}")
                if original is not None:
                    raise RuntimeError(f"native reduce continued after an exception: {line}")
                try:
                    if line == "next":
                        if iterator is None:
                            iterator = iter(source)
                        try:
                            value = next(iterator)
                        except StopIteration:
                            token = -1
                        else:
                            token = len(objects)
                            objects.append(value)
                    elif line.startswith("call "):
                        _, left, right = line.split()
                        value = function(objects[int(left)], objects[int(right)])
                        token = len(objects)
                        objects.append(value)
                    else:
                        raise RuntimeError(f"unexpected native request: {line}")
                except Exception as exc:
                    original = exc
                    token = -2
                proc.stdin.write(f"{token}\n")
                proc.stdin.flush()
            tail, errors = proc.communicate(timeout=15)
            if proc.returncode or tail or errors:
                raise RuntimeError(f"native reduce exited {proc.returncode}: {tail!r} {errors!r}")
        finally:
            if proc.poll() is None:
                proc.kill()
    if line == "remote exception" and original is not None:
        raise original
    if original is not None:
        raise RuntimeError(f"native reduce lost the fixture exception: {line}")
    if line.startswith("type-error "):
        raise TypeError(line.removeprefix("type-error "))
    return objects[int(line.removeprefix("result "))]


def adapter(binary, counts, *, pure):
    missing = object()

    def invoke(function, iterable, /, *args, initial=missing):
        if args:
            if len(args) != 1 or initial is not missing:
                raise TypeError("reduce requires at most one initial value")
            return native_call(binary, "positional", function, iterable, args[0], counts)
        mode = "unseeded" if initial is missing else "keyword"
        return native_call(binary, mode, function, iterable, initial, counts)

    def reduce(function, sequence, initial=missing):
        return invoke(function, sequence, initial=initial)

    def entry(*args, **kwargs):
        before = counts["native_calls"]
        deprecated = pure and ("function" in kwargs or "sequence" in kwargs)
        try:
            if deprecated:
                # The unaccelerated 3.14 wrapper warns before its Python argument
                # binder runs. The bound arguments still go to the native fold.
                counts["host_binding"] += 1
                counts["host_warnings"] += 1
                warnings.warn('Calling functools.reduce with keyword arguments '
                              '"function" or "sequence" is deprecated in Python 3.14 '
                              'and will be forbidden in Python 3.16.',
                              DeprecationWarning, stacklevel=2)
                return reduce(*args, **kwargs)
            return invoke(*args, **kwargs)
        finally:
            if not deprecated and counts["native_calls"] == before:
                counts["host_binding"] += 1

    return staticmethod(entry)


def keyword_controls(binary, module):
    """Compare the two original variants with the adapters before patching either one."""
    values = [1, 2, 3, 4, 5]
    calls = {
        "function_and_sequence": ((), dict(function=add, sequence=values, initial=1)),
        "sequence": ((add,), dict(sequence=values, initial=1)),
        "initial": ((add, values), dict(initial=1)),
    }

    def observe(fn, args, kwargs):
        with warnings.catch_warnings(record=True) as events:
            warnings.simplefilter("always")
            try:
                result = {"value": fn(*args, **kwargs)}
            except TypeError as exc:
                result = {"exception": type(exc).__name__}
        return {**result, "warnings": [(w.category.__name__, str(w.message)) for w in events]}

    rows = {}
    for name, fn, pure in (("python", module.py_functools.reduce, True),
                           ("accelerator", module.c_functools.reduce, False)):
        rows[name] = {}
        for spelling, (args, kwargs) in calls.items():
            counts = Counter()
            reference = observe(fn, args, kwargs)
            actual = observe(adapter(binary, counts, pure=pure).__func__, args, kwargs)
            if actual != reference:
                raise AssertionError((name, spelling, reference, actual))
            rows[name][spelling] = {"reference": reference, "adapter": actual, "counts": dict(counts)}
    return rows


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for arg in ("imre", "runtime", "clang", "result"):
        p.add_argument("--" + arg, required=True, type=Path)
    args = p.parse_args()
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        p.error("test setup needs CPython 3.14.5")
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    inventory = json.loads((HERE / "reference-linux.json").read_text())
    report = {"module": manifest["module"], "cpython_commit": manifest["commit"],
              "denominator": inventory["denominator"], "optimization_levels": {}, "keyword_controls": {}}
    with tempfile.TemporaryDirectory() as temp:
        for level in (0, 3):
            binary = Path(temp) / f"reduce-{level}"
            subprocess.run([args.imre.resolve(), "build", HERE / "reduce_driver.py", "-o", binary,
                            "--opt-level", str(level), "--no-cache", "--runtime", args.runtime.resolve(),
                            "--clang", args.clang.resolve()], check=True, timeout=120)
            discovered = {case.id(): case for case in cases(load_suite(manifest["module"]))}
            if discovered.keys() != inventory["reference"].keys():
                raise ValueError("suite IDs differ from the pinned reference")
            module = sys.modules[manifest["module"]]
            report["keyword_controls"][str(level)] = keyword_controls(binary, module)
            selected = {n: c for n, c in discovered.items()
                        if n.split(".")[-2] in {"TestReduceC", "TestReducePy"}}
            tools = Toolchain(args.imre, args.runtime, args.clang, level)
            native, attempts, results = {}, {}, {}
            path = HERE / "upstream/Lib/test/test_functools.py"
            for name, case in selected.items():
                declaring = next(cls for cls in type(case).__mro__ if case._testMethodName in vars(cls))
                directory = Path(temp) / f"{level}-{name}"
                directory.mkdir()
                # Preserve the actual class fixture's callable as a static public operation.
                fixture = ("import functools\nimport unittest\n\nclass NativeCase(unittest.TestCase):\n"
                           "    reduce = staticmethod(functools.reduce)\n")
                try:
                    native[name] = native_method(tools, directory, path, declaring.__name__, name, fixture)
                except RuntimeError as exc:
                    attempts[name] = {"status": "compiler_blocked",
                                      "diagnostic": str(exc).strip().splitlines()[-1].replace(str(directory), "<case>")}
            evidence, failures, skips = run_in_fixtures(load_suite(manifest["module"]), native)
            if skips:
                raise RuntimeError(f"unexpected native skips: {skips}")
            for name, entries in evidence.items():
                if entries:
                    body, = entries
                    results[name] = {"status": "native_body_host_fixture", **body}
            for name, message in failures:
                attempts[name] = {"status": "native_failed", "diagnostic": message.strip().splitlines()[-1]}

            counts = {n: Counter() for n in selected if n not in results}
            def exercise(case, original):
                counter = counts[case.id()]
                pure = case.reduce is module.py_functools.reduce
                with patch.object(type(case), "reduce", adapter(binary, counter, pure=pure)):
                    original()
                if not counter["native_calls"]:
                    raise ValueError(f"no native operation: {case.id()}")
                state = "mixed" if counter["host_binding"] else "host_driven_native_operation"
                return {"status": state, **counter}

            evidence, failures, skips = run_in_fixtures(
                load_suite(manifest["module"]), dict.fromkeys(counts, exercise))
            if skips:
                raise RuntimeError(f"unexpected host skips: {skips}")
            for name, entries in evidence.items():
                if entries:
                    entry, = entries
                    results[name] = {**entry, "complete_native_body": attempts[name]}
            for name, message in failures:
                results[name] = {"status": "failed", **counts[name], "diagnostic": message.strip().splitlines()[-1],
                                 "complete_native_body": attempts[name]}
            if selected.keys() != results.keys():
                raise RuntimeError("missing reduce outcomes")
            results = dict(sorted(results.items()))
            report["optimization_levels"][str(level)] = results
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    return int(any(row["status"] == "failed"
                   for level in report["optimization_levels"].values() for row in level.values()))


if __name__ == "__main__":
    sys.exit(main())
