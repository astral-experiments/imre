"""Verify the cumulative warnings ledger against the intact and selected receipts."""
import ast
import csv
import hashlib
import json
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


def main():
    original = subprocess.check_output([sys.executable, "-I", "-B", BASE / "verify.py"])
    assert original == (BASE / "parents.tsv").read_bytes()
    baseline = rows(BASE / "parents.tsv")
    current = rows(ROOT / "parents.tsv")
    assert len(current) == len(baseline) == 187
    selected = []
    for prior, row in zip(baseline, current):
        assert all(row[k] == prior[k] for k in ("ordinal", "id", "occurrence"))
        if row["receipt"] == "dfe0f18e":
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
