"""Validate accepted omission policies against pinned originals and receipts."""
import ast
import csv
import hashlib
import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
FULL = HERE.parent / "cpython_stdlib/full"
ARGPARSE = FULL / "argparse"
BASE64 = FULL / "base64"
BASE64_REVISION = "8571ca2dbecabf8d7863c369dc776611fd57d8ad"
BASE64_STAGE = HERE / "revisions" / BASE64_REVISION
BASE64_POLICY = BASE64_STAGE / "omissions/base64"


def require(condition, message):
    if not condition:
        raise ValueError(message)


def calls(node, name):
    return [n for n in ast.walk(node) if isinstance(n, ast.Call) and ast.unparse(n.func) == name]


def read_policy(directory):
    schema = (FULL / "re/pending-omissions-linux.tsv").read_text().splitlines()[0].split("\t")
    with (directory / "accepted-omissions-linux.tsv").open(newline="") as f:
        reader = csv.DictReader(f, delimiter="\t")
        require(reader.fieldnames == schema, "omission ledger schema differs from review convention")
        return list(reader)


def pinned_source(directory, source_path, commit):
    data = (directory / "upstream" / source_path).read_bytes()
    sha256 = hashlib.sha256(data).hexdigest()
    blob = hashlib.sha1(f"blob {len(data)}\0".encode() + data).hexdigest()
    manifest = json.loads((directory / "UPSTREAM.json").read_text())
    require(manifest["commit"] == commit and manifest["files"][source_path] == blob, f"{directory.name} source is not the pinned blob")
    return data, sha256, blob


def check_identity(row, key, receipt, inventory, node, source_path, sha256, blob):
    commit = inventory["cpython_commit"]
    ids = list(inventory["reference"])
    ordinal = int(row["ordinal"])
    require(row["decision"] == "accepted" and row["cpython_commit"] == commit and row["cpython_source_path"] == source_path and row["source_sha256"] == sha256, f"wrong policy source or decision: {key}")
    require(receipt["source"] == {"repo": "https://github.com/python/cpython", "commit": commit, "path": source_path, "sha256": sha256, "git_blob": blob} and not receipt["paired"], f"policy disagrees with canonical receipt or credits omission: {key}")
    require(1 <= ordinal <= len(ids) and ids[ordinal - 1] == key[0] and inventory["reference"][key[0]] == "pass" and node is not None and (node.lineno, node.end_lineno) == (int(row["def_line"]), int(row["end_line"])), f"wrong original identity or source span: {key}")


def validate_argparse():
    rows = read_policy(ARGPARSE)
    inventory = json.loads((ARGPARSE / "reference-linux.json").read_text())
    ids = list(inventory["reference"])
    receipts = {(r["id"], r["occurrence"]): r for r in map(json.loads, (HERE / "stdlib/argparse.jsonl").read_text().splitlines())}
    with (ARGPARSE / "paired-8730.tsv").open(newline="") as f:
        paired = {r["id"] for r in csv.DictReader(f, delimiter="\t") if all(r[t] == "pass" for t in ("stock", "O0", "O3"))}
    with (FULL / "frozen-stock-intersection-8818.tsv").open(newline="") as f:
        historical = next(r for r in csv.DictReader(f, delimiter="\t") if r["family"] == "argparse")
    require(len(ids) == inventory["denominator"] == int(historical["originals"]) and len(paired) == int(historical["paired"]), "argparse historical counts differ from inventory")

    source_path = "Lib/test/test_argparse.py"
    commit = inventory["cpython_commit"]
    data, sha256, blob = pinned_source(ARGPARSE, source_path, commit)
    cls = next(n for n in ast.parse(data).body if isinstance(n, ast.ClassDef) and n.name == "TestProgName")
    methods = {n.name: n for n in cls.body if isinstance(n, ast.FunctionDef)}
    steps = [calls(methods["make_script"], name) for name in ("py_compile.compile", "os.remove", "import_helper.make_legacy_pyc")]
    require(all(len(step) == 1 for step in steps) and steps[0][0].lineno < steps[1][0].lineno < steps[2][0].lineno and calls(methods["check_usage"], "script_helper.assert_python_ok"), "compiled fixture no longer creates bytecode, removes source, and launches the child")
    compiled = {"test.test_argparse.TestProgName." + n for n in methods if n.endswith("_compiled")}
    actual = set()
    for row in rows:
        key = (row["full_id"], int(row["occurrence"]))
        require(key not in actual and key in receipts and all(row.values()), f"invalid or duplicate omission {key}")
        actual.add(key)
        receipt = receipts[key]; method = row["full_id"].rsplit(".", 1)[-1]; node = methods.get(method)
        check_identity(row, key, receipt, inventory, node, source_path, sha256, blob)
        counterpart = method.removesuffix("_compiled"); wrapper = calls(node, "self." + counterpart)
        require(len(wrapper) == 1 and any(k.arg == "compiled" and isinstance(k.value, ast.Constant) and k.value.value is True for k in wrapper[0].keywords) and calls(methods[counterpart], "self.make_script") and calls(methods[counterpart], "self.check_usage"), f"wrong compiled fixture path: {key}")
        witness = (key[0].removesuffix("_compiled"), 1)
        require(row["separately_covered_witness_keys"] == f"{witness[0]}#1" and witness in receipts and witness[0] in paired, f"wrong source counterpart: {key}")
    require({k[0] for k in actual} == compiled == set(ids) - paired and len(actual) == 6, "accepted omissions must be the six exact historical argparse gaps")
    return len(rows)


def constant_args(call):
    return [arg.value for arg in call.args if isinstance(arg, ast.Constant)]


def validate_base64():
    rows = read_policy(BASE64_POLICY)
    inventory = json.loads((BASE64 / "reference-linux.json").read_text())
    staged = [json.loads(line) for line in (BASE64_STAGE / "stdlib/base64.jsonl").read_text().splitlines()]
    receipts = {(r["id"], r["occurrence"]): r for r in staged}
    require(len(receipts) == len(staged) and all(r["imre_revision"] == BASE64_REVISION and r["family"] == "base64" for r in staged), "base64 staged receipt revision, family, or occurrence differs")
    require(len(inventory["reference"]) == inventory["denominator"] == len(staged) == len(receipts) == 41 and set(receipts) == {(test_id, 1) for test_id in inventory["reference"]}, "base64 pinned inventory or staged occurrences differ")
    source_path = "Lib/test/test_base64.py"
    data, sha256, blob = pinned_source(BASE64, source_path, inventory["cpython_commit"])
    cls = next(n for n in ast.parse(data).body if isinstance(n, ast.ClassDef) and n.name == "TestMain")
    methods = {n.name: n for n in cls.body if isinstance(n, ast.FunctionDef)}
    helper = calls(methods["get_output"], "script_helper.assert_python_ok")
    require(len(helper) == 1 and constant_args(helper[0]) == ["-m", "base64"], "base64 helper is no longer an interpreter module launch")
    # Method, interpreter launch, assertion names and constant assertions, stream API witness.
    expected = {
        "test_decode": ("self.get_output", ["-d"], [("self.assertEqual", [b"a\xffb"])], "test_decode"),
        "test_encode_file": ("self.get_output", ["-e"], [("self.assertEqual", [b"Yf9iCg=="])], "test_encode"),
        "test_encode_from_stdin": ("script_helper.spawn_python", ["-m", "base64", "-e"], [("self.assertEqual", [b"Yf9iCg=="]), ("self.assertIsNone", [])], "test_encode"),
        "test_prints_usage_with_help_flag": ("self.get_output", ["-h"], [("self.assertIn", [b"usage: "]), ("self.assertIn", [b"-d, -u: decode"])], None),
        "test_prints_usage_with_invalid_flag": ("script_helper.assert_python_failure", ["-m", "base64", "-x"], [("self.assertIn", [b"usage: "]), ("self.assertIn", [b"-d, -u: decode"])], None),
    }
    require({n for n in methods if n.startswith("test_")} == set(expected), "base64 TestMain method set changed")
    actual = set()
    for row in rows:
        key = (row["full_id"], int(row["occurrence"]))
        method = key[0].removeprefix("test.test_base64.TestMain.")
        node = methods.get(method)
        require(key not in actual and key in receipts and method in expected and key[1] == 1, f"invalid or duplicate base64 omission: {key}")
        actual.add(key)
        require(all(row[k] for k in row if k not in {"class_or_method_decorator_lines", "separately_covered_witness_keys"}) and not row["class_or_method_decorator_lines"] and not cls.decorator_list and not node.decorator_list, f"invalid base64 policy fields or decorators: {key}")
        receipt = receipts[key]
        check_identity(row, key, receipt, inventory, node, source_path, sha256, blob)
        require(receipt["runs"]["stock"]["outcome"] == "pass" and all(receipt["runs"][t]["outcome"] == "not_run" and receipt["runs"][t].get("reason") == "original deliberately excluded because it launches a child interpreter" for t in ("O0", "O3")) and receipt["omission_reason"] is None, f"base64 staged raw outcome or exclusion reason changed: {key}")
        launch, args, assertions, witness = expected[method]
        launches = calls(node, launch)
        observed = sorted((n for n in ast.walk(node) if isinstance(n, ast.Call) and ast.unparse(n.func).startswith("self.assert")), key=lambda n: n.lineno)
        require(len(launches) == 1 and constant_args(launches[0]) == args and [(ast.unparse(n.func), constant_args(n)) for n in observed] == assertions, f"base64 launch or individual assertions changed: {key}")
        witness_key = f"test.test_base64.LegacyBase64TestCase.{witness}#1" if witness else "none"
        require(row["separately_covered_witness_keys"] == witness_key, f"wrong stream witness or invented CLI witness: {key}")
        if witness:
            witness_id = witness_key.rsplit("#", 1)[0]
            record = receipts.get((witness_id, 1))
            require(record is not None and record["id"] == witness_id and record["occurrence"] == 1 and record["imre_revision"] == BASE64_REVISION and record["source"] == receipt["source"] and record["paired"] is True and all(record["runs"][tier]["outcome"] == "pass" for tier in ("stock", "O0", "O3")), f"stream witness lacks the exact staged source, paired flag, or pass observations: {key}")
    require({k[0] for k in actual} == {"test.test_base64.TestMain." + n for n in expected} and len(actual) == 5, "base64 policy must cover exactly five TestMain originals")
    return len(rows)


def validate():
    return validate_argparse() + validate_base64()


if __name__ == "__main__":
    print(f"validated {validate()} accepted omissions (6 argparse, 5 base64); raw denominators retained")
