"""Verify a complete selected native-child warnings group from an unpacked pairing/ tree."""
import ast
import collections
import hashlib
import json
from pathlib import Path


def require(condition, message):
    if not condition:
        raise ValueError(message)


def read(path):
    return json.loads(path.read_bytes())


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def lines(path):
    return enumerate(path.read_text().splitlines(), 1)


def callbacks(folder, label, expected):
    """Return each parent's ordered raw event and subtest line references."""
    path = folder / (label + ".stdout")
    raw = [(n, ln.split("\t", 2)) for n, ln in lines(path)]
    record = read(folder / (label + ".json"))
    for kind in ("stdout", "stderr"):
        require(digest(folder / (label + "." + kind)) == record[kind + "_sha256"], str(path))
    require(record["guard"] is None and record["rc"] in (0, 1), str(path))
    def events(kind):
        return [parts[1:] for _, parts in raw if parts[0] == kind]
    require(events("REGISTER") == [[key, ""] for key in expected], "registration order")
    require(events("COUNT") == [["registered", str(len(expected))]], "suite count")
    require(events("END") == [["testsRun", str(len(expected))]] and raw[-1][1][0] == "END", "suite end")
    require(events("PROVIDER_IDENTICAL") == [["C/PurePython", "False"]], "module identity")
    p = {k: ast.literal_eval(v) for k, v in events("PROVIDER")}
    require(len(events("PROVIDER")) == 3 and {k: (v[0], v[2], v[3]) for k, v in p.items()} == {
        "original": ("warnings", "builtin_function_or_method", "_warnings"),
        "C": ("warnings", "builtin_function_or_method", "_warnings"),
        "PurePython": ("_py_warnings", "function", "_py_warnings"),
    }, "provider fingerprints")
    require(events("EXECUTABLE") == [[record["argv"][0]]], "foreign top-level interpreter")
    require([row[0] for row in events("START")] == expected and [row[0] for row in events("STOP")] == expected, "execution order")
    active = None
    by_key = {}
    for n, parts in raw:
        kind = parts[0]
        if kind in {"EXECUTABLE", "PROVIDER", "PROVIDER_IDENTICAL", "COUNT", "END"}:
            continue
        require(len(parts) == 3 and parts[1] in expected, f"{path}:{n} invalid callback")
        _, key, detail = parts
        if kind == "REGISTER":
            require(active is None and key not in by_key and len(by_key) < len(expected), "late registration")
            by_key[key] = {"lines": {"REGISTER": n}, "subtests": []}
            continue
        if kind == "START":
            require(active is None and len(by_key) == len(expected), "nested start")
            active = key
        require(active == key, f"{path}:{n} unbalanced parent")
        r = by_key[key]
        if kind in {"SUBPASS", "SUBFAIL"}:
            r["subtests"].append([kind, ast.literal_eval(detail), n])
        else:
            require(kind not in r["lines"], "repeated " + kind)
            r["lines"][kind] = n
            if kind == "STOP":
                active = None
            elif kind != "START":
                require(kind in {"PASS", "FAIL", "ERROR", "SKIP", "XFAIL", "XPASS"} and "result" not in r, "invalid parent outcome")
                r.update(result=kind, detail=detail)
    for key, r in by_key.items():
        if "result" not in r:
            require(any(item[0] == "SUBFAIL" for item in r["subtests"]), "missing result for " + key)
            r["result"] = "SUBFAIL"
    require(active is None, "unterminated parent")
    unsuccessful = any(r["result"] in {"ERROR", "FAIL", "XPASS", "SUBFAIL"} or any(kind == "SUBFAIL" for kind, _, _ in r["subtests"]) for r in by_key.values())
    require(record["rc"] == int(unsuccessful), "exit code inconsistent with callbacks")
    return record, by_key


def audit_native(folder, data, tools, expected, level, o0_hash):
    """Join process-group PID observations to raw LLVM compile events by parent phase."""
    require(digest(folder / "executions.jsonl") == data["audit_sha256"], "clang event digest")
    require(digest(folder / "clang-audit.sh") == data["clang_wrapper_sha256"], "clang audit wrapper digest")
    monitor = read(folder / "process-group.json")
    driver = data["argv"][0]
    top = monitor["group"]
    need_hash = o0_hash if level == "O0" else tools["files"]["imre"]
    require(data["head"] == tools["head"] and data["level"] == level and data["driver_sha256"] == need_hash, "driver freeze")
    require(data["runtime_sha256"] == tools["files"]["libimre_runtime.a"], "runtime freeze")
    require(monitor["driver_path"] == driver and top == monitor["parent_pid"] and not monitor["guard"] and not monitor["foreign"], "process-group guard")
    phases = {"startup", *expected}
    drivers = collections.defaultdict(dict)
    workload_pids = collections.defaultdict(set)
    top_seen = False
    for n, line in lines(folder / "process-group.jsonl"):
        event = json.loads(line)
        kind, phase, pid = event["kind"], event["phase"], event["pid"]
        require(kind in ("native-driver", "linked-program") and phase in phases, "unexpected executable")
        if kind != "native-driver":
            continue
        require(event["argv0"] == driver, "foreign driver")
        if pid == top:
            require(phase == "startup", "top-level driver attributed to a child")
            top_seen = True
        else:
            # The observer can record multiple exec images for one PID. Keep all raw lines.
            drivers[phase].setdefault(pid, []).append(n)
            # Pinned script_helper prefixes workload invocations with "-X faulthandler".
            # Its optional "-E -c pass" interpreter probe is a separate child.
            if event["options"][:1] == ["-X"]:
                workload_pids[phase].add(pid)
    require(top_seen, "missing top-level driver")
    require({phase: sorted(pids) for phase, pids in drivers.items() if pids} == monitor["observed_driver_children"], "process group summary disagrees with raw images")
    raw_clang = {tuple(row.split("\t")): n for n, row in lines(folder / "clang.tsv")}
    require(len(raw_clang) == len((folder / "clang.tsv").read_text().splitlines()), "duplicate clang row")
    used = set()
    codegen = collections.defaultdict(dict)
    for n, line in lines(folder / "executions.jsonl"):
        e = json.loads(line)
        if "clang" not in e:
            continue
        phase = e["parent"]
        invocation = tuple(e["clang"])
        require(phase in phases and len(invocation) == 4 and invocation in raw_clang and invocation not in used, "clang audit rows")
        used.add(invocation)
        clang_pid, caller_pid, flag, ir = invocation
        if ir == "yes":
            require(flag == "-" + level, "wrong native opt level")
            codegen[phase].setdefault(int(caller_pid), []).append({"pid": int(clang_pid), "executions_line": n, "clang_line": raw_clang[invocation]})
    require(used == set(raw_clang), "unattributed clang invocation")
    require(top in codegen["startup"], "top-level LLVM compile")
    require({phase: ["-" + level] for phase, pids in codegen.items() if pids} == data["observed_codegen"], "codegen summary disagrees with raw events")
    evidence = {}
    for key in expected:
        group, compilers = drivers[key], codegen[key]
        # The pinned Bootstrap and late-resource tests each call assert_python_ok
        # twice. test_tracemalloc calls its local wrapper twice. The remaining
        # 16 original methods each make one assert_python_ok/failure call.
        required = 2 if key.removeprefix("test.test_warnings.") in {
            "BootstrapTest.test_issue_8766", "FinalizationTest.test_late_resource_warning",
            "PyWarningsDisplayTests.test_tracemalloc",
        } else 1
        evidence[key] = {"required_workload_children": required,
                         "matched_workload_pids": sorted(workload_pids[key] & compilers.keys()),
                         "matched": [{"driver_pid": pid, "group_lines": group[pid], "codegen": compilers[pid]} for pid in sorted(group.keys() & compilers.keys())],
                         "unmatched_drivers": sorted(group.keys() - compilers.keys()),
                         "unobserved_codegen_drivers": sorted(compilers.keys() - group.keys())}
    return evidence


def validate_group(root, tag, label, expected_head):
    root = Path(root)
    baseline = read(root / "baseline.json")
    inventory = read(root / "original-187.json")
    require(digest(root / "original-187.json") == baseline["inventory_sha256"] and inventory["n"] == 187, "wrong stock inventory")
    for source, fingerprint in baseline["sources"].items():
        require(digest(root / "pinned-source" / source) == fingerprint, "source drift in " + source)
    selections = read(root / "selection.json")
    expected, child_ids = selections[label], selections["selected_children"]
    require(len(set(child_ids)) == len(child_ids) == 19 and expected and len(set(expected)) == len(expected)
            and set(expected) <= set(child_ids) and all([k, 1] in inventory["keys"] for k in expected), "invalid child selection")
    tagdir = root / tag
    tools = read(tagdir / "tools.json")
    require(tools["head"] == expected_head, "unexpected public revision")
    hashes = {Path(path).name: h for h, path in [x.split(None, 1) for x in (tagdir / "child-tools/sha256-O0.txt").read_text().splitlines()]}
    require(hashes["libimre_runtime.a"] == tools["files"]["libimre_runtime.a"], "O0 library mismatch")
    o0_build = (tagdir / "child-tools/cargo-O0.log").read_text()
    # O0 changes only the command's build-time default; both drivers use the
    # exact runtime from the public freeze identified by tools.json.
    require("Finished " in o0_build and str(Path(tools["repo"]) / "crates/imre-compiler") in o0_build,
            "O0 compiler build was not recorded from the frozen checkout")

    suffix = "" if label == "selected_children" else "-" + label
    rows = {k: {} for k in expected}
    for level in ("stock", "O0", "O3"):
        folder = root / "stock/stock" if level == "stock" else tagdir / ("children-" + level + suffix)
        proc, parents = callbacks(folder, label, expected)
        require(proc["argv"][-1] == label and proc["runner_sha256"] == digest(root / "launcher/runner.py"), "wrong launcher")
        if level == "stock":
            require(proc["command_sha256"] == baseline["stock_executable_sha256"], "unverified stock executable")
            require(all(r["result"] == "PASS" for r in parents.values()), "expected a pinned stock PASS for each child parent")
        else:
            require(proc["selection_sha256"] == digest(root / (label + ".py")), "wrong selected fixture")
            proofs = audit_native(folder, proc, tools, expected, level, hashes["imre"])
        for key in expected:
            row = parents[key]
            row["stdout"] = str((folder / (label + ".stdout")).relative_to(root))
            if level != "stock":
                row["children"] = proofs[key]
            rows[key][level] = row
    for key, row in rows.items():
        stock_sub = [x[:2] for x in row["stock"]["subtests"]]
        row["paired"] = all(row[level]["result"] == "PASS" and [x[:2] for x in row[level]["subtests"]] == stock_sub for level in ("stock", "O0", "O3")) and all(x[0] == "SUBPASS" for x in stock_sub)
        for level in ("O0", "O3"):
            ev = row[level]["children"]
            row["paired"] = row["paired"] and len(ev["matched_workload_pids"]) >= ev["required_workload_children"] and not ev["unmatched_drivers"] and not ev["unobserved_codegen_drivers"]
    return rows
