"""Validate accepted argparse omission policy against pinned originals and receipts."""
import ast
import csv
import hashlib
import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
FULL = HERE.parent / "cpython_stdlib/full"
ARGPARSE = FULL / "argparse"
POLICY = ARGPARSE / "accepted-omissions-linux.tsv"


def require(condition, message):
    if not condition:
        raise ValueError(message)


def calls(node, name):
    return [n for n in ast.walk(node) if isinstance(n, ast.Call) and ast.unparse(n.func) == name]


def validate():
    # Reuse the per-original omission ledger schema already used for regex review.
    schema = (FULL / "re/pending-omissions-linux.tsv").read_text().splitlines()[0].split("\t")
    with POLICY.open(newline="") as f:
        reader = csv.DictReader(f, delimiter="\t")
        require(reader.fieldnames == schema, "omission ledger schema differs from review convention")
        rows = list(reader)
    inventory = json.loads((ARGPARSE / "reference-linux.json").read_text())
    ids = list(inventory["reference"])
    receipts = {(r["id"], r["occurrence"]): r for r in map(json.loads, (HERE / "stdlib/argparse.jsonl").read_text().splitlines())}
    with (ARGPARSE / "paired-8730.tsv").open(newline="") as f:
        paired = {r["id"] for r in csv.DictReader(f, delimiter="\t") if all(r[t] == "pass" for t in ("stock", "O0", "O3"))}
    with (FULL / "frozen-stock-intersection-8818.tsv").open(newline="") as f:
        historical = next(r for r in csv.DictReader(f, delimiter="\t") if r["family"] == "argparse")
    require(len(ids) == inventory["denominator"] == int(historical["originals"]) and len(paired) == int(historical["paired"]), "argparse historical counts differ from inventory")

    source_path = "Lib/test/test_argparse.py"
    data = (ARGPARSE / "upstream" / source_path).read_bytes()
    commit = inventory["cpython_commit"]
    sha256 = hashlib.sha256(data).hexdigest()
    blob = hashlib.sha1(f"blob {len(data)}\0".encode() + data).hexdigest()
    manifest = json.loads((ARGPARSE / "UPSTREAM.json").read_text())
    require(manifest["commit"] == commit and manifest["files"][source_path] == blob, "argparse source is not the pinned blob")
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
        require(row["decision"] == "accepted" and row["cpython_commit"] == commit and row["cpython_source_path"] == source_path and row["source_sha256"] == sha256, f"wrong policy source or decision: {key}")
        require(receipt["source"] == {"repo": "https://github.com/python/cpython", "commit": commit, "path": source_path, "sha256": sha256, "git_blob": blob} and not receipt["paired"], f"policy disagrees with canonical receipt or credits omission: {key}")
        require(ids[int(row["ordinal"]) - 1] == key[0] and inventory["reference"][key[0]] == "pass" and node is not None and (node.lineno, node.end_lineno) == (int(row["def_line"]), int(row["end_line"])), f"wrong original identity or source span: {key}")
        counterpart = method.removesuffix("_compiled"); wrapper = calls(node, "self." + counterpart)
        require(len(wrapper) == 1 and any(k.arg == "compiled" and isinstance(k.value, ast.Constant) and k.value.value is True for k in wrapper[0].keywords) and calls(methods[counterpart], "self.make_script") and calls(methods[counterpart], "self.check_usage"), f"wrong compiled fixture path: {key}")
        witness = (key[0].removesuffix("_compiled"), 1)
        require(row["separately_covered_witness_keys"] == f"{witness[0]}#1" and witness in receipts and witness[0] in paired, f"wrong source counterpart: {key}")
    require({k[0] for k in actual} == compiled == set(ids) - paired and len(actual) == 6, "accepted omissions must be the six exact historical argparse gaps")
    return len(rows)


if __name__ == "__main__":
    print(f"validated {validate()} accepted argparse omissions; raw denominator retained")
