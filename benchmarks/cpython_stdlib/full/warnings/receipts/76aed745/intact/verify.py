"""Verify every original callback and native child in the intact public 76aed745 run."""
import ast
import csv
import hashlib
import json
import runpy
import tarfile
import tempfile
from pathlib import Path


HERE = Path(__file__).resolve().parent
RECEIPTS = HERE.parents[1]
HEAD = "76aed745516103a63aea7e19fb4f22bf5b0fc774"
TAG = "public-76aed745"
LABEL = "intact_fixture"


def verify(root):
    audit = runpy.run_path(str(RECEIPTS / "dfe0f18e-children/audit.py"))
    read = audit["read"]
    digest = audit["digest"]
    baseline = read(root / "baseline.json")
    for path, expected_hash in baseline["sources"].items():
        assert digest(root / "pinned-source" / path) == expected_hash
    inventory = read(root / "original-187.json")["keys"]
    assert digest(root / "original-187.json") == baseline["inventory_sha256"]
    with (RECEIPTS / "parents.tsv").open() as stream:
        ledger = list(csv.DictReader(stream, delimiter="\t"))
    assert [[row["id"], int(row["occurrence"])] for row in ledger] == inventory
    names = [row["id"] for row in ledger]
    assert len(set(names)) == len(names) == 187
    expected_children = read(root / "selection.json")["selected_children"]
    assert len(set(expected_children)) == len(expected_children) == 19
    source = root / TAG
    tools = read(source / "tools.json")
    assert tools["head"] == HEAD and tools["tree"] == "32a90213719174222ba8516cb6226f1ece6517fa"
    fixture = root / (LABEL + ".py")
    assert digest(fixture) == "a0e9537c28f2041703b084a2646fe1aa803937b8f45ecdc6bd48c29332ef370f"

    stock_proc, stock = audit["callbacks"](root / "stock/stock", LABEL, names)
    assert stock_proc["rc"] == 0 and stock_proc["argv"][-1] == LABEL
    assert stock_proc["command_sha256"] == baseline["stock_executable_sha256"]
    runner = digest(root / "launcher/runner.py")
    assert stock_proc["runner_sha256"] == runner
    assert sum(len(row["subtests"]) for row in stock.values()) == 18
    assert all(s[0] == "SUBPASS" for row in stock.values() for s in row["subtests"])
    gates = {r["id"] for r in read(RECEIPTS / "cpython-only-gates.json")["gates"]}
    stock_skips = {name for name, row in stock.items() if row["result"] == "SKIP"}
    assert stock_skips == set(baseline["stock_skips"]) and len(stock_skips) == 6
    assert len(gates) == 4 and gates.isdisjoint(stock_skips)
    traced = "test.test_warnings.PyWarningsDisplayTests.test_tracemalloc"
    trace = {traced: {"paired": True}}
    for level in ("O0", "O3"):
        folder = source / f"children-{level}-{LABEL}"
        process, native = audit["callbacks"](folder, LABEL, names)
        hash_file = source / f"child-tools/sha256-{level}.txt"
        hashes = {Path(path).name: sha for sha, path in
                  (line.split(None, 1) for line in hash_file.read_text().splitlines())}
        assert process["rc"] == 0 and process["argv"][-1] == LABEL
        assert process["selection_sha256"] == digest(fixture) and process["runner_sha256"] == runner
        assert process["driver_sha256"] == hashes["imre"]
        assert process["runtime_sha256"] == hashes["libimre_runtime.a"]
        evidence = audit["audit_native"](folder, process, tools, expected_children, level, hashes["imre"])
        commands = [json.loads(line) for line in (folder / "process-group.jsonl").read_text().splitlines()]
        for event in commands:
            if event["kind"] != "native-driver":
                continue
            argv = event["argv"]
            assert argv[0] == event["argv0"] == process["argv"][0]
            assert hashlib.sha256("\0".join(argv).encode()).hexdigest() == event["argv_sha256"]
            if event["phase"] == "startup":
                assert argv == process["argv"]
            elif event["pid"] in evidence[event["phase"]]["matched_workload_pids"]:
                assert argv[1:3] == ["-X", "faulthandler"]
            else:
                assert argv[1:] == ["-E", "-c", "import sys; sys.exit(0)"]
        for children in evidence.values():
            assert not children["unmatched_drivers"] and not children["unobserved_codegen_drivers"]
            assert len(children["matched_workload_pids"]) == children["required_workload_children"]
        trace[traced][level] = {"children": evidence[traced]}

        for row in ledger:
            name = row["id"]
            upstream, result = stock[name], native[name]
            assert upstream["result"] == row["stock"] and result["result"] == row[level]
            assert [s[:2] for s in result["subtests"]] == [s[:2] for s in upstream["subtests"]]
            if name in stock_skips:
                assert result["detail"] == upstream["detail"] and row["disposition"] == "stock-skip"
            elif name in gates:
                assert ast.literal_eval(result["detail"]) == "implementation detail specific to cpython"
                assert row["disposition"] == "native-skip"
            else:
                assert result["result"] == upstream["result"] == "PASS" and row["disposition"] == "paired"

        matched = sum(len(e["matched"]) for e in evidence.values())
        print(f"{level}: 177 passes, 10 skips, 18 subtests, {matched} matched child driver PIDs")
    runpy.run_path(str(RECEIPTS / "76aed745/workloads.py"))["verify"](root, TAG, LABEL, trace)


def main():
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        for directory in (RECEIPTS / "dfe0f18e", HERE.parent, HERE):
            with tarfile.open(directory / "raw.tar.gz") as archive:
                archive.extractall(root, filter="data")
        verify(root)


if __name__ == "__main__":
    main()
