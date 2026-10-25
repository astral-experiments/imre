"""Compile unchanged CPython JSON test methods, their JSON calls and assertions."""

import argparse
import ast
import json
from pathlib import Path
import sys
import tempfile
import textwrap

sys.path.insert(0, str(Path(__file__).resolve().parent))
from reference import BENCH, COMMIT, routine, verify

sys.path.insert(0, str(BENCH))
from suite_inventory import cases, load_suite
from suite_native import Toolchain, method_bytes, native_method, run_in_fixtures
from roundtrips import ASSERT_EQUAL, definitions as roundtrip_definitions
from sweep import prepare

# Signatures specialize only the fixture boundary, never the upstream method.
# Counts describe the unchanged source, including the complete Unicode escape sweep.
GROUPS = {
    ("test_unicode", "TestUnicode", "str", "str", "str"): {
        "test_encoding3": 1, "test_encoding5": 1, "test_encoding7": 1,
        "test_ascii_non_printable_encode": 2, "test_big_unicode_encode": 2,
        "test_big_unicode_decode": 2, "test_unicode_decode": 0xd7ff,
    },
    ("test_unicode", "TestUnicode", "list[str]", "str", "str"): {
        "test_encoding4": 1, "test_encoding6": 1,
    },
    ("test_decode", "TestDecode", "int", "bytes", "int"): {"test_bytes": 1},
    ("test_dump", "TestDump", "dict[str, int]", "str", "str"): {"test_dumps": 1},
}


def fixture(encode, input_type, result_type):
    return f"""import json

class NativeCase:
    def assertEqual(self, left: {result_type}, right: {result_type}) -> None:
        if left != right:
            raise AssertionError("native operands differ")
    def loads(self, value: {input_type}) -> {result_type}:
        return json.loads(value)
    def dumps(self, value: {encode}, ensure_ascii: bool = True) -> str:
        return json.dumps(value, ensure_ascii=ensure_ascii)

"""


def compiled(tools, directory, fixture_ids, *, provenance=False):
    """Build one full definition and execute the binary under each inherited fixture."""
    for (module, owner, encode, input_type, result_type), methods in GROUPS.items():
        path = BENCH / f"upstream/Lib/test/test_json/{module}.py"
        for method, assertions in methods.items():
            source = f"test.test_json.{module}.{owner}.{method}"
            ids = fixture_ids[source]
            work = directory / source
            work.mkdir()
            options = {}
            if provenance:
                tree = ast.parse(textwrap.dedent(method_bytes(path, owner, method).decode()))
                calls = sorted({"json." + n.func.attr for n in ast.walk(tree)
                                if isinstance(n, ast.Call) and isinstance(n.func, ast.Attribute)
                                and isinstance(n.func.value, ast.Name) and n.func.value.id == "self"
                                and n.func.attr in ("loads", "dumps", "load", "dump")})
                if not calls:
                    raise ValueError(f"no JSON call in {source}")
                options["operation"] = ("imre", ", ".join(calls))
            callback = native_method(tools, work, path, owner, ids[0],
                                     fixture(encode, input_type, result_type), **options)
            yield source, ids, assertions, callback

    # These complete definitions use the bundled unittest.TestCase unchanged.
    for module, owner, method, count, operations in (
        ("test_fail", "TestFail", "test_failures", 34, "json.loads: 34 checker inputs"),
        ("test_decode", "TestDecode", "test_constant_invalid_case", 6, "json.loads: 6 invalid constants"),
        ("test_decode", "TestDecode", "test_nonascii_digits_rejected", 3, "json.loads: 3 non-ASCII numerals"),
        ("test_float", "TestFloat", "test_ints", 6, "json.dumps: twice per integer, including 2**64"),
    ):
        source = f"test.test_json.{module}.{owner}.{method}"
        ids = fixture_ids[source]
        path = BENCH / f"upstream/Lib/test/test_json/{module}.py"
        work = directory / source
        work.mkdir()
        options = {"operation": ("imre", operations)} if provenance else {}
        callback = native_method(tools, work, path, owner, ids[0], prepare(path, owner, method), **options)
        yield source, ids, count, callback

    for _, owner, path, source in roundtrip_definitions():
        ids = fixture_ids[source]
        work = directory / source
        work.mkdir()
        options = {"operation": ("imre", "json.loads, json.dumps, json.loads; parsed-value equality")} if provenance else {}
        callback = native_method(tools, work, path, owner, ids[0],
                                 prepare(path, owner, "test_parse") + ASSERT_EQUAL, **options)
        yield source, ids, 1, callback


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    verify()
    module_name = "test.test_json"
    fixture_ids = {}
    for case in cases(load_suite(module_name)):
        fixture_ids.setdefault(routine(case), []).append(case.id())
    report = {"cpython_commit": COMMIT, "execution": "native body, JSON calls and assertions",
              "fixture_execution": "original CPython module/class/instance hooks; native child instance",
              "routines": {}}
    ok = True
    for level in (0, 3):
        tools = Toolchain(args.imre, args.runtime, args.clang, level)
        with tempfile.TemporaryDirectory() as tmp:
            executions = {}
            for key, ids, assertions, callback in compiled(tools, Path(tmp), fixture_ids):
                executions.update(dict.fromkeys(ids, callback))
                report["routines"].setdefault(key, {"source_assertions": assertions,
                                                    "fixture_ids": ids, "levels": {}})
            evidence, failures, skips = run_in_fixtures(load_suite(module_name), executions)
            if failures or skips or set(evidence) != executions.keys():
                ok = False
                print(f"level {level}: {failures=} {skips=}", file=sys.stderr)
            for key, row in report["routines"].items():
                row["levels"][str(level)] = {
                    name: evidence.get(name, []) for name in fixture_ids[key]
                }
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    return int(not ok)


if __name__ == "__main__":
    sys.exit(main())
