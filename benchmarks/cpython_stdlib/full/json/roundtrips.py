"""Isolate the three JSON_checker round-trip failures under both original fixtures."""

import argparse
from collections import defaultdict
import hashlib
import importlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from reference import BENCH, COMMIT, routine, verify
from sweep import diagnostic, digest, prepare

sys.path.insert(0, str(BENCH))
from suite_inventory import cases, load_suite
from suite_native import NativeCompileError, Toolchain, method_bytes, native_method, run_in_fixtures


ASSERT_EQUAL = """    def assertEqual(self, first, second, msg: str | None = None) -> None:
        if first != second:
            raise AssertionError("unequal JSON documents")
"""
# Each pair changes one nested leaf. These controls are outside the suite denominator.
UNEQUAL = (('[1, {"k": 1}]', '[1, {"k": 2}]'), ('[[["a"]]]', '[[["b"]]]'),
           ('{"a": {"b": "c"}}', '{"a": {"b": "d"}}'))


def output(binary):
    child = subprocess.run([binary], capture_output=True, check=True, timeout=60)
    if child.stderr:
        raise ValueError(child.stderr)
    return child.stdout


def run(tools, work):
    ids = defaultdict(list)
    for case in cases(load_suite("test.test_json")):
        ids[routine(case)].append(case.id())
    methods = {}
    for n in (1, 2, 3):
        module, owner = f"test.test_json.test_pass{n}", f"TestPass{n}"
        path = BENCH / f"upstream/Lib/test/test_json/test_pass{n}.py"
        source = f"{module}.{owner}.test_parse"
        names = ids[source]
        if len(names) != 2 or len(set(names)) != 2:
            raise ValueError(f"expected two distinct inherited IDs: {names}")
        base = prepare(path, owner, "test_parse")
        row = methods[source] = {
            "method_sha256": hashlib.sha256(method_bytes(path, owner, "test_parse")).hexdigest(),
            "fixture_ids": names,
        }
        for name, fixture in (("inherited_assertion", base), ("document_assertion", base + ASSERT_EQUAL)):
            directory = work / f"{owner}-{name}"
            directory.mkdir()
            try:
                callback = native_method(tools, directory, path, owner, names[0], fixture)
            except NativeCompileError as exc:
                row[name] = {"compile_error": diagnostic(exc, directory)}
                continue
            passed, failed, skipped = run_in_fixtures(load_suite("test.test_json"), dict.fromkeys(names, callback))
            if skipped or sorted([*passed, *(key for key, _ in failed)]) != sorted(names):
                raise ValueError((passed, failed, skipped))
            row[name] = {key: "pass" for key in passed}
            row[name].update((key, msg.splitlines()[-1]) for key, msg in failed)

        directory = work / f"{owner}-serialization"
        directory.mkdir()
        # Print the result after the original loads/dumps/loads sequence.
        program = base + "\nprint(json.dumps(json.loads(json.dumps(json.loads(JSON)))))\n"
        actual = output(tools.build(program.encode(), directory))
        expected = json.dumps(json.loads(importlib.import_module(module).JSON)).encode() + b"\n"
        if actual != expected:
            raise ValueError(f"native round-trip output differs from the pinned reference: {source}")
        row["matching_serialized_output_sha256"] = hashlib.sha256(actual).hexdigest()

    directory = work / "negative-controls"
    directory.mkdir()
    fixture = "import json\nimport unittest\nclass NativeCase(unittest.TestCase):\n" + ASSERT_EQUAL
    program = fixture + "\ncase = NativeCase()\n"
    for left, right in UNEQUAL:
        program += (f"try:\n    case.assertEqual(json.loads({left!r}), json.loads({right!r}))\n"
                    'except AssertionError:\n    print("caught")\n'
                    'else:\n    raise ValueError("accepted unequal documents")\n')
    try:
        binary = tools.build(program.encode(), directory)
    except NativeCompileError as exc:
        controls = {"compile_error": diagnostic(exc, directory)}
    else:
        if output(binary) != b"caught\n" * len(UNEQUAL):
            raise ValueError("missing native AssertionError in a negative control")
        controls = {"unequal_pairs_rejected": len(UNEQUAL)}
    return {"methods": methods, "negative_controls": controls}


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        p.add_argument("--" + name, required=True, type=Path)
    p.add_argument("--imre-revision", required=True, help="Git commit used to build the supplied artifacts")
    args = p.parse_args()
    verify()
    report = {"cpython_commit": COMMIT, "imre_commit": args.imre_revision,
              "sha256": {"imre": digest(args.imre), "runtime": digest(args.runtime)},
              "fixture": "CPython module/class/instance lifecycle; Imre instance, body, JSON operations and assertions",
              "levels": {}}
    for level in (0, 3):
        with tempfile.TemporaryDirectory() as directory:
            tools = Toolchain(args.imre.resolve(), args.runtime.resolve(), args.clang.resolve(), level)
            report["levels"][str(level)] = run(tools, Path(directory))
    args.result.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
