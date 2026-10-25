"""Run all seven original reduce routines with Imre performing the left fold."""
import argparse
from collections import Counter
import json
from pathlib import Path
import select
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_inventory import cases, load_suite, verify_files


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


def adapter(binary, counts):
    missing = object()

    def invoke(function, iterable, /, *args, initial=missing):
        if args:
            if len(args) != 1 or initial is not missing:
                raise TypeError("reduce requires at most one initial value")
            return native_call(binary, "positional", function, iterable, args[0], counts)
        mode = "unseeded" if initial is missing else "keyword"
        return native_call(binary, mode, function, iterable, initial, counts)

    def entry(*args, **kwargs):
        before = counts["native_calls"]
        try:
            return invoke(*args, **kwargs)
        finally:
            if counts["native_calls"] == before:
                counts["host_binding"] += 1

    return staticmethod(entry)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for arg in ("imre", "runtime", "clang", "result"):
        p.add_argument("--" + arg, required=True, type=Path)
    args = p.parse_args()
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        p.error("test setup needs CPython 3.14.5")
    import test
    verify_files(HERE / "upstream", manifest["files"])
    verify_files(Path(test.__file__).resolve().parents[2], {
        k: v for k, v in manifest["files"].items() if k.startswith("Lib/test/")})
    inventory = json.loads((HERE / "reference-linux.json").read_text())
    report = {"module": manifest["module"], "cpython_commit": manifest["commit"],
              "denominator": inventory["denominator"], "optimization_levels": {}}
    with tempfile.TemporaryDirectory() as temp:
        for level in (0, 3):
            binary = Path(temp) / f"reduce-{level}"
            subprocess.run([args.imre.resolve(), "build", HERE / "reduce_driver.py", "-o", binary,
                            "--opt-level", str(level), "--no-cache", "--runtime", args.runtime.resolve(),
                            "--clang", args.clang.resolve()], check=True, timeout=120)
            discovered = {case.id(): case for case in cases(load_suite(manifest["module"]))}
            if discovered.keys() != inventory["reference"].keys():
                raise ValueError("suite IDs differ from the pinned reference")
            results = {}
            for name, case in discovered.items():
                if name.split(".")[-2] not in {"TestReduceC", "TestReducePy"}:
                    continue
                counts = Counter()
                with patch.object(type(case), "reduce", adapter(binary, counts)):
                    result = unittest.TestResult()
                    case.run(result)
                if result.errors or result.failures:
                    state = "failed"
                    details = (result.errors or result.failures)[0][1].strip().splitlines()[-1]
                elif result.skipped or not counts["native_calls"]:
                    raise ValueError(f"unexpected skip or reference-only result: {name}")
                else:
                    state = "mixed" if counts["host_binding"] else "host_driven_native_operation"
                    details = None
                results[name] = {"status": state, **dict(counts)}
                if details:
                    results[name]["diagnostic"] = details
            report["optimization_levels"][str(level)] = results
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    return int(any(row["status"] == "failed"
                   for level in report["optimization_levels"].values() for row in level.values()))


if __name__ == "__main__":
    sys.exit(main())
