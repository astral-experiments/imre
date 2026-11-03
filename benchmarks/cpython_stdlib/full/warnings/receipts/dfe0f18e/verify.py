"""Reconstruct parents.tsv from the archived stock and public native callbacks."""

import ast
import collections
import csv
import hashlib
import io
import json
import re
import sys
import tarfile
from pathlib import Path


HERE = Path(__file__).resolve().parent
TAG = "public-dfe0f18e"
HEAD = "dfe0f18ebecfc5b302a5747f79079d48b9eb86eb"
TREE = "10c9946decb8a304cc62bdd9267497e37f73238b"
INVENTORY = "e052c72bf31c8b98636aae6da3cd9d8266ab4e991f5410753a05e80760ef48ff"
SUITE = "3b00dac0c24bbaf28b3f442402c95d7850f50b567f94d8378d950513150e0943"
PARENT_EVENTS = {"REGISTER", "START", "PASS", "SKIP", "FAIL", "ERROR", "XFAIL", "XPASS", "STOP"}
CHILD_CALL = re.compile(r"\b(assert_python_(ok|failure)|subprocess|Popen)\b|sys\.executable")


def digest(data):
    return hashlib.sha256(data).hexdigest()


def load_stream(files, folder, keys):
    stem = folder + "/intact_fixture"
    proc = json.loads(files[stem + ".json"])
    assert proc["rc"] in (0, 1) and proc["guard"] is None
    assert digest(files[stem + ".stdout"]) == proc["stdout_sha256"]
    assert digest(files[stem + ".stderr"]) == proc["stderr_sha256"]
    assert proc["runner_sha256"] == digest(files["launcher/runner.py"])
    events = [line.split("\t", 2) for line in files[stem + ".stdout"].decode().splitlines()]
    executable = proc["argv"][0] if folder == "stock/stock" else "/usr/bin/python3"
    assert [row for row in events if row[0] == "EXECUTABLE"] == [["EXECUTABLE", executable]]
    providers = {row[1]: ast.literal_eval(row[2]) for row in events if row[0] == "PROVIDER"}
    for name, module, kind, origin in (
        ("C", "warnings", "builtin_function_or_method", "_warnings"),
        ("PurePython", "_py_warnings", "function", "_py_warnings"),
    ):
        mod, _, typ, owner, context = providers[name]
        assert (mod, typ, owner, context) == (module, kind, origin, False)
    assert [row for row in events if row[0] == "PROVIDER_IDENTICAL"] == [
        ["PROVIDER_IDENTICAL", "C/PurePython", "False"]
    ]
    assert [row for row in events if row[0] == "COUNT"] == [["COUNT", "registered", str(len(keys))]]
    assert events[-1] == ["END", "testsRun", str(len(keys))]
    for label in ("REGISTER", "START", "STOP"):
        assert [row[1] for row in events if row[0] == label] == keys

    tracks = {key: [] for key in keys}
    subtests = {key: [] for key in keys}
    active = None
    for lineno, row in enumerate(events, 1):
        if row[0] == "START":
            assert active is None
            active = row[1]
        if row[0] in (PARENT_EVENTS - {"REGISTER"}) or row[0] in {"SUBPASS", "SUBFAIL"}:
            assert row[1] == active
        if row[0] == "STOP":
            active = None
        if row[0] in PARENT_EVENTS:
            tracks[row[1]].append((row[0], lineno))
        elif row[0] in ("SUBPASS", "SUBFAIL"):
            subtests[row[1]].append((row[0], row[2]))
    for key, track in tracks.items():
        labels = [label for label, _ in track]
        assert labels[:2] == ["REGISTER", "START"] and labels[-1] == "STOP", key
    return tracks, subtests


def main():
    with tarfile.open(HERE / "raw.tar.gz") as archive:
        files = {item.name: archive.extractfile(item).read() for item in archive if item.isfile()}
    sums = {}
    for row in files.pop("SHA256SUMS").decode().splitlines():
        checksum, name = row.split("  ", 1)
        sums[name] = checksum
    assert {name: digest(data) for name, data in files.items()} == sums
    assert digest(files["original-187.json"]) == INVENTORY
    inventory = json.loads(files["original-187.json"])["keys"]
    keys = [name for name, occurrence in inventory if occurrence == 1]
    assert len(set(keys)) == len(keys) == len(inventory) == 187
    metadata = list(csv.DictReader(io.StringIO(files["stock-parents.tsv"].decode()), delimiter="\t"))
    assert [[row["canonical_id"], int(row["occurrence"])] for row in metadata] == inventory

    tools = json.loads(files[TAG + "/tools.json"])
    assert (tools["head"], tools["tree"]) == (HEAD, TREE)
    compiled = json.loads(files[TAG + "/compiled-sha256.json"])
    assert compiled["frozen"] == tools["files"]
    baseline = json.loads(files["baseline.json"])
    assert baseline["stock_revision"] == "5607950ef232dad16d75c0cf53101d9649d89115"
    assert baseline["warnings_omissions"] == 0
    for name, expected in baseline["sources"].items():
        assert digest(files["pinned-source/" + name]) == expected
    suite = files["pinned-source/Lib/test/test_warnings/__init__.py"]
    assert digest(suite) == SUITE
    src = suite.decode().splitlines()
    stock_proc = json.loads(files["stock/stock/intact_fixture.json"])
    assert stock_proc["command_sha256"] == baseline["stock_executable_sha256"]
    for level in ("O0", "O3"):
        prefix = TAG + "/" + level
        build = json.loads(files[prefix + "/build.json"])
        assert build["rc"] == 0 and build["guard"] is None
        assert build["command_sha256"] == tools["files"]["imre"]
        run = json.loads(files[prefix + "/intact_fixture.json"])
        assert run["command_sha256"] == compiled["launchers"][level]
        argv = build["argv"]
        assert Path(argv[argv.index("--runtime") + 1]) == Path(argv[0]).with_name("libimre_runtime.a")
        assert argv[argv.index("--opt-level") + 1] == level[1]
        assert argv[argv.index("-o") + 1] == run["argv"][0]
    streams = [load_stream(files, folder, keys) for folder in ("stock/stock", TAG + "/O0", TAG + "/O3")]
    writer = csv.writer(sys.stdout, delimiter="\t", lineterminator="\n")
    writer.writerow(("ordinal", "id", "occurrence", "disposition", "stock", "O0", "O3",
                     "stock_lines", "O0_lines", "O3_lines"))
    counts = collections.Counter()
    child_ids = set()
    skips = []
    for row in metadata:
        name = row["canonical_id"]
        tracks = [events[name] for events, _ in streams]
        outcomes = ["/".join(label for label, _ in track[2:-1]) for track in tracks]
        children = [subtests[name] for _, subtests in streams]
        method = src[int(row["source_first"]) - 1:int(row["source_last"])]
        if CHILD_CALL.search("\n".join(method)):
            child_ids.add(name)
        assert outcomes[0] == row["result"]
        if outcomes[0] == "SKIP":
            disposition = "stock-skip"
            skips.append(name)
        elif name in child_ids:
            disposition = "requires-native-child"
        elif outcomes == ["PASS"] * 3 and children[0] == children[1] == children[2]:
            assert all(label == "SUBPASS" for label, _ in children[0])
            disposition = "paired"
        elif "SKIP" in outcomes[1:]:
            disposition = "native-skip"
        else:
            disposition = "native-nonpass"
        counts[disposition] += 1
        lines = ["/".join(str(line) for _, line in track) for track in tracks]
        writer.writerow((row["ordinal"], name, row["occurrence"], disposition, *outcomes, *lines))
    assert skips == baseline["stock_skips"] and len(skips) == 6
    assert child_ids == set(json.loads(files["selection.json"])["selected_children"]) and len(child_ids) == 19
    print(dict(counts), file=sys.stderr)


if __name__ == "__main__":
    main()
