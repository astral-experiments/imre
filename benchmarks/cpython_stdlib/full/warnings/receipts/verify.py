"""Verify the cumulative warnings ledger against the intact and selected receipts."""
import ast
import csv
import hashlib
import json
import runpy
import tarfile
import tempfile
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent
BASE = ROOT / "dfe0f18e"


def sha256(data):
    return hashlib.sha256(data.encode()).hexdigest()


def rows(path):
    with path.open() as stream:
        return list(csv.DictReader(stream, delimiter="\t"))


def native_children(receipt, label, head):
    tools = ROOT / "dfe0f18e-children/audit.py"
    validate = runpy.run_path(str(tools))["validate_group"]
    tag = "public-" + head[:8]
    with tempfile.TemporaryDirectory() as scratch:
        for archive in (BASE / "raw.tar.gz", ROOT / receipt / "raw.tar.gz"):
            with tarfile.open(archive) as raw:
                raw.extractall(scratch, filter="data")
        result = validate(scratch, tag, label, head)
        if receipt == "061b0196":
            audit = runpy.run_path(str(ROOT / receipt / "workloads.py"))["verify"]
            audit(Path(scratch), tag, label, result)
        return result


def child_row(previous, result, receipt):
    row = dict(previous, receipt=receipt)
    row["disposition"] = "paired" if result["paired"] else "native-child-nonpass"
    for mode in ("stock", "O0", "O3"):
        record = result[mode]
        row[mode] = record["result"]
        labels = ("REGISTER", "START", record["result"], "STOP")
        row[mode + "_lines"] = "/".join(str(record["lines"][name]) for name in labels)
    return row


def environment_children():
    with tempfile.TemporaryDirectory() as scratch:
        with tarfile.open(ROOT / "58f847de/raw.tar.gz") as packet:
            packet.extractall(scratch, filter="data")
        audit = runpy.run_path(str(ROOT / "58f847de/audit.py"))["verify"]
        verified = audit(Path(scratch))
        assert verified["public_head"] == "58f847de7c7c1630fd19446a538058b99673a571"
        return {(r["id"], r["occurrence"]): r["receipts"] for r in verified["parents"]}


def replayed_child_row(previous, receipts):
    row = dict(previous, receipt="58f847de", disposition="paired")
    for mode in ("stock", "O0", "O3"):
        row[mode] = "PASS"
        row[mode + "_lines"] = "/".join(map(str, receipts[mode]["lines"]))
    return row


def main():
    original = subprocess.check_output([sys.executable, "-I", "-B", BASE / "verify.py"])
    assert original == (BASE / "parents.tsv").read_bytes()
    baseline = rows(BASE / "parents.tsv")
    current = rows(ROOT / "parents.tsv")
    assert len(current) == len(baseline) == 187
    selected = []
    children = native_children("dfe0f18e-children", "selected_children",
                               "dfe0f18ebecfc5b302a5747f79079d48b9eb86eb")
    shutdown = native_children("061b0196", "selected_shutdown_children",
                               "061b01963d06311f964de28ecf655176265f66c0")
    assert len(shutdown) == 2 and all(r["paired"] for r in shutdown.values())
    assert set(children) == {r["id"] for r in baseline if r["disposition"] == "requires-native-child"}
    replayed = environment_children()
    assert len(replayed) == 4 and all(k in children and n == 1 for k, n in replayed)
    for prior, row in zip(baseline, current):
        assert all(row[k] == prior[k] for k in ("ordinal", "id", "occurrence"))
        if prior["id"] in children:
            first = child_row(prior, children[prior["id"]], "dfe0f18e-children")
            key = (prior["id"], int(prior["occurrence"]))
            if first["disposition"] != "paired" and key in replayed:
                assert row == replayed_child_row(first, replayed[key])
            elif first["disposition"] != "paired" and prior["id"] in shutdown:
                assert row == child_row(first, shutdown[prior["id"]], "061b0196")
            else:
                assert row == first
        elif row["receipt"] == "dfe0f18e":
            assert all(row[k] == value for k, value in prior.items())
        else:
            assert row["receipt"] == "7e46a592"
            assert prior["disposition"] == "native-nonpass" and row["disposition"] == "paired"
            selected.append(row)
    raw = json.loads((ROOT / "7e46a592/raw.json").read_bytes())
    assert raw["head"] == "7e46a5929afff9a96d78324b28608570b633f6c8"
    assert raw["source_sha256"] == "3b00dac0c24bbaf28b3f442402c95d7850f50b567f94d8378d950513150e0943"
    names = [row["id"] for row in selected]
    assert len(names) == 3
    for mode in ("stock", "O0", "O3"):
        run = raw["runs"][mode]
        process = run["process"]
        assert process["rc"] == 0 and process["guard"] is None and not process["descendants"]
        assert sha256("".join(run["stdout"])) == process["stdout_sha256"]
        assert sha256(run["stderr"]) == process["stderr_sha256"]
        assert sha256(raw["runner"]) == process["runner_sha256"]
        if mode != "stock":
            build = run["build"]
            argv = build["argv"]
            assert build["rc"] == 0 and build["command_sha256"] == raw["toolchain"]["files"]["imre"]
            assert argv[argv.index("--opt-level") + 1] == mode[1:]
            assert argv[argv.index("-o") + 1] == process["argv"][0]
        events = [line.rstrip("\n").split("\t") for line in run["stdout"]]
        providers = {e[1]: ast.literal_eval(e[2]) for e in events if e[0] == "PROVIDER"}
        assert providers["C"][2:] == ("builtin_function_or_method", "_warnings", False)
        assert providers["PurePython"][2:] == ("function", "_py_warnings", False)
        assert ["PROVIDER_IDENTICAL", "C/PurePython", "False"] in events
        assert [event[1] for event in events if event[0] == "REGISTER"] == names
        assert events[-1] == ["END", "testsRun", str(len(names))]
        for row in selected:
            name = row["id"]
            trace = [(index, event[0]) for index, event in enumerate(events, 1) if event[1] == name]
            assert [label for _, label in trace] == ["REGISTER", "START", "PASS", "STOP"]
            assert [line for line, _ in trace] == [int(n) for n in row[mode + "_lines"].split("/")]
            assert row[mode] == "PASS"
    paired = sum(row["disposition"] == "paired" for row in current)
    print(f"{paired} paired / {len(current)} original parents")


if __name__ == "__main__":
    main()
