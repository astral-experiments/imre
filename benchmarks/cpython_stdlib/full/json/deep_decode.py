"""Exercise both original high-nesting decoder fixtures, including every native exception."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from reference import BENCH, COMMIT, routine, verify
sys.path.insert(0, str(BENCH))
from suite_inventory import cases, load_suite
from suite_native import Toolchain, run_in_fixtures

METHOD = "test.test_json.test_recursion.TestRecursion.test_highly_nested_objects_decoding"
DRIVER = b'''import json
import sys
text = open("document.txt").read()
try:
    root = json.loads(text)
    path = sys.argv[1]
    print("OK")
    if path == "@repr":
        print(repr(root))
    elif path == "@equal":
        print(root == json.loads(text))
        print(root == json.loads(text.replace("315", "316")))
    elif path == "@add":
        combined = root[0] + json.loads('[[]]')[0]
        print(len(combined))
    elif path:
        item = root[0] if path[0] == "a" else root["key"]
        for step in path[1:]:
            if step == "a":
                item = item[0]
            else:
                item = item["key"]
        leaf: int = item
        print(leaf)
except RecursionError as e:
    print("RecursionError")
    print(str(e))
except json.JSONDecodeError as e:
    print("JSONDecodeError")
    print(e.msg)
    print(e.pos)
    print(e.lineno)
    print(e.colno)
    print(e.doc == text)
    print(str(e))
'''


def native(program, work, text, path=""):
    (work / "document.txt").write_text(text)
    run = subprocess.run([program, path], cwd=work, capture_output=True, check=True, timeout=30)
    if run.stderr:
        raise ValueError(run.stderr)
    return run.stdout.decode().splitlines()


def fixture(program, work):
    """Bind the original loads calls to one implementation and retain each exception."""
    def execute(case, original):
        observed = []
        reference = case.loads

        def loads(text):
            call = {"input_sha256": hashlib.sha256(text.encode()).hexdigest(), "characters": len(text)}
            observed.append(call)
            if program is None:
                try:
                    return reference(text)
                except RecursionError as exc:
                    call.update(exception=type(exc).__name__, message=str(exc))
                    raise
            outcome = native(program, work, text)
            if len(outcome) == 2 and outcome[0] == "RecursionError":
                call.update(exception=outcome[0], message=outcome[1])
                raise RecursionError(outcome[1])
            raise RuntimeError(f"unexpected native JSON result: {outcome}")

        case.loads = loads
        original()
        if len(observed) != 3:
            raise ValueError(observed)
        return observed
    return execute


def original_fixtures(program, work):
    ids = [c.id() for c in cases(load_suite("test.test_json")) if routine(c) == METHOD]
    if len(ids) != 2:
        raise ValueError(ids)
    execute = fixture(program, work)
    complete, failed, skipped = run_in_fixtures(load_suite("test.test_json"), dict.fromkeys(ids, execute))
    if failed or skipped or set(complete) != set(ids):
        raise ValueError((failed, skipped, complete))
    return {name: records[0] for name, records in complete.items()}


def nested(path, leaf):
    return ("".join("[" if step == "a" else '{"key":' for step in path) + leaf
            + "".join("]" if step == "a" else "}" for step in reversed(path)))


def controls(program, work):
    results = []
    for pattern, depth in (("a", 129), ("o", 257), ("ao", 4096), ("oa", 8192)):
        path = (pattern * depth)[:depth]
        text = nested(path, "315")
        value = json.loads(text)
        for step in path:
            value = value[0] if step == "a" else value["key"]
        if native(program, work, text, path) != ["OK", str(value)]:
            raise ValueError((pattern, depth))
        results.append({"depth": depth, "path": pattern, "leaf": value})
    for pattern, depth in (("a", 50_000), ("oa", 30_000)):
        path = (pattern * depth)[:depth]
        text = nested(path, "315")
        reference = json.loads(text)
        expected = repr(reference)
        if native(program, work, text, "@repr") != ["OK", expected]:
            raise ValueError(("deep repr", pattern, depth))
        if native(program, work, text, "@equal") != ["OK", "True", "False"]:
            raise ValueError(("deep equality", pattern, depth))
        if pattern == "a" and native(program, work, text, "@add") != ["OK", str(len(reference[0] + []))]:
            raise ValueError(("deep add", pattern, depth))
        results.append({"depth": depth, "path": pattern, "repr_sha256": hashlib.sha256(expected.encode()).hexdigest(),
                        "equal_and_unequal": True, "list_add": pattern == "a"})
    for text in (nested("a"*135, "[0,]"), nested("o"*135, '{"é":"🌗",}'),
                 nested("ao"*132, '{"é"\n 0}'), nested("ao"*132, '"bad\\y"'),
                 nested("oa"*100, "1e"), nested("oa"*180, "null")+" x"):
        try:
            json.loads(text)
        except json.JSONDecodeError as e:
            expected = ["JSONDecodeError", e.msg, str(e.pos), str(e.lineno), str(e.colno), "True", str(e)]
        else:
            raise ValueError("malformed control accepted by the reference")
        if native(program, work, text) != expected:
            raise ValueError((expected, native(program, work, text)))
        results.append({"input_sha256": hashlib.sha256(text.encode()).hexdigest(), "exception": expected})
    return results


def main():
    from evidence import definition
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        p.add_argument("--"+name, required=True, type=Path)
    args = p.parse_args()
    verify()
    report = {"cpython_commit": COMMIT, "method": METHOD, "method_sha256": definition(METHOD)[1],
              "fixture": "CPython lifecycle, decorators, recursion context, method and assertions",
              "operation": "each json.loads in the native run executes in Imre",
              "reference_only": original_fixtures(None, None), "levels": {}}
    for level in (0, 3):
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory)
            tools = Toolchain(args.imre.resolve(), args.runtime.resolve(), args.clang.resolve(), level)
            program = tools.build(DRIVER, work)
            report["levels"][str(level)] = {"fixtures": original_fixtures(program, work),
                                            "controls": controls(program, work)}
    args.result.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
