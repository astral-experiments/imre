"""Compare six source-bound conformance scanners and their complete reports on pinned real inputs."""

from __future__ import annotations

import argparse
from collections import Counter
from datetime import datetime, timezone
import hashlib
import json
import os
import platform
from pathlib import Path
import random
import re
import runpy
import statistics
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
VALIDATION = runpy.run_path(str(ROOT / "benchmarks/conformance_validate.py"))
ENGINES = ("upstream", "cpython", "before", "after", "codon", "rust")
PANELS = ("unchanged-github", "changed-github", "changed-diff")
REVISION = "835b70f3f795f8e061b1f7621178515b0a77a0ad"


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def file_id(path: Path) -> dict:
    return {"path": str(path), "bytes": path.stat().st_size, "sha256": sha(path.read_bytes())}


def orders() -> list[list[str]]:
    """A complete even Williams design balances positions and directed neighbors."""
    n = len(ENGINES)
    first = [0, 1, -1, 2, -2, 3]
    rows = [[ENGINES[(start + x) % n] for x in first] for start in range(n)]
    assert all(len({row[i] for row in rows}) == n for i in range(n))
    assert Counter(pair for row in rows for pair in zip(row, row[1:])) == {
        (a, b): 1 for a in ENGINES for b in ENGINES if a != b
    }
    return rows


def observations(lines: list[str], pattern) -> tuple[bytes, int, int]:
    dump, count, checksum = [], 0, 0
    for number, line in enumerate(lines):
        if match := re.search(pattern, line):
            start, end = match.span()
            optional, multi = int(match["optional"] is not None), int(match["multi"] is not None)
            tag, description = match["tag"] or "", match["description"] or "Missing"
            count += 1
            checksum += 1 + start + 3 * end + 5 * len(description) + 7 * len(tag) + 11 * optional + 13 * multi
            dump.append(f"{number}:{start}:{end}:{optional}:{multi}:{len(tag)}:{tag}{description}")
    dump.append(f"{count} {checksum}")
    return ("\n".join(dump) + "\n").encode(), count, checksum


def prepare(directory: Path, captures: Path) -> dict:
    provenance = json.loads((captures / "preparation.json").read_bytes())
    manifest = json.loads((captures / "manifest.json").read_bytes())
    suite = Path(provenance["suite_directory"])
    assert provenance["guards_passed"] and provenance["suite_revision"] == manifest["suite_revision"] == REVISION
    assert subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=suite, text=True).strip() == REVISION
    paths = {k: captures / (k + ".json") for k in ("manifest", "old", "new")}
    cases = [Path(f) for f in manifest["test_files"]]
    assert len(set(cases)) == len(cases) == 146
    for file in cases:
        assert sha(file.read_bytes()) == provenance["source_sha256"][str(file.relative_to(suite))]
    for role in ("old", "new"):
        assert sha(paths[role].read_bytes()) == provenance["captures"][role]["stdout_sha256"]
    lines = [line for file in cases for line in file.read_text(encoding="utf-8").splitlines()]
    rng = random.Random(20261022)
    space = [chr(i) for i in range(sys.maxunicode + 1) if chr(i).isspace() and chr(i) != "\r"]
    edges = ["", "missing", "# E", "# E[tag+]", "# E?[]: malformed", "æ𝄞# E", "é # E: café", "# E:"]
    for ws in space:
        edges.extend([f"𐐷#{ws}E?[𐐷+]{ws}:{ws}é", f"x #{ws}no # E[]", f"# E{ws}:é"])
    for _ in range(600):
        edges.append(rng.choice(["", "# ", "λ ", "x # bad ", "𐐷"]) + "#" + rng.choice(space)
                     + rng.choice(["E", "E?", "EE", "X"]) + rng.choice(["", "[+]", "[]", "[ô+]", "[x++]", "[???]"])
                     + rng.choice(["", ":", ": question? ", "\t:🐈", "    "]))
    pattern = VALIDATION["upstream"]().CONFORMANCE_ERROR_PATTERN
    fixtures = {}
    for name, values in (("corpus", lines), ("edge", edges)):
        assert all("\0" not in line for line in values)
        path = directory / (name + ".txt")
        path.write_bytes("\0".join(values).encode())
        stdout, matches, checksum = observations(values, pattern)
        dump = directory / (name + ".captures")
        dump.write_bytes(stdout)
        fixtures[name] = {"input": file_id(path), "capture_oracle": file_id(dump),
                          "lines": len(values), "matches": matches, "checksum": checksum}
    return {"revision": REVISION, "suite": str(suite), "test_files": [file_id(p) for p in cases],
            "captures": {k: file_id(p) for k, p in paths.items()},
            "preparation": file_id(captures / "preparation.json"), "fixtures": fixtures}


def main() -> None:
    if sys.flags.optimize or sys.gettrace() or sys.getprofile():
        raise RuntimeError("timing requires ordinary Python with assertions enabled")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", required=True, type=Path, help="retained engine commands, executable and source identities")
    parser.add_argument("--captures", type=Path, required=True, help="pinned changed GitHub report captures")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--cpu", type=int, required=True)
    parser.add_argument("--samples", type=int, default=12)
    parser.add_argument("--scan-rounds", type=int, default=64)
    parser.add_argument("--report-rounds", type=int, default=4)
    parser.add_argument("--report-panels", nargs="+", choices=PANELS, default=PANELS)
    parser.add_argument("--pilot", action="store_true")
    args = parser.parse_args()
    if args.samples <= 0 or (not args.pilot and args.samples % len(ENGINES)) or min(args.scan_rounds, args.report_rounds) < 1:
        parser.error("positive samples/rounds required; full runs must complete the balanced design")
    directory = args.output.resolve()
    directory.mkdir(parents=True, exist_ok=False)
    cfg = json.loads(args.config.read_bytes())
    assert set(cfg["engines"]) == set(ENGINES)
    assert args.pilot or (cfg["source_locked"] and tuple(args.report_panels) == PANELS)
    assert cfg["revision"] == subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    for value in cfg["guarded_inputs"]:
        assert file_id(Path(value["path"])) == value
    affinity = sorted(os.sched_getaffinity(0))
    os.sched_setaffinity(0, {args.cpu})
    inputs = prepare(directory, args.captures.resolve())
    panels, full, reports = {}, {}, {}
    for panel in args.report_panels:
        location = args.captures.resolve().parents[1] / panel / "captures"
        paths = {role: location / (role + ".json") for role in ("manifest", "old", "new")}
        prep = json.loads((location / "preparation.json").read_bytes())
        manifest = json.loads(paths["manifest"].read_bytes())
        assert prep["guards_passed"] and prep["suite_revision"] == manifest["suite_revision"] == REVISION
        assert manifest["test_files"] == [f["path"] for f in inputs["test_files"]]
        for role in ("old", "new"):
            assert sha(paths[role].read_bytes()) == prep["captures"][role]["stdout_sha256"]
        workload = VALIDATION["Workload"](paths["manifest"], paths["old"], paths["new"], panel.split("-")[1])
        report = VALIDATION["oracle"](workload).report
        assert report is not None
        target = directory / (panel + ".md")
        target.write_bytes(report.encode())
        full[panel], reports[panel] = workload, report
        panels[panel] = {"captures": {key: file_id(path) for key, path in paths.items()},
                         "preparation": file_id(location / "preparation.json"),
                         "expected_report": {**file_id(target), "characters": len(report)}}
    env = VALIDATION["environment"](directory / "home")
    env["PYTHONPATH"] = ""
    (directory / "home").mkdir()
    by_case, plan = {}, orders()
    for name, rounds in (("scanner-preload", 0), ("scanner", args.scan_rounds)):
        by_case[name] = {"executable": "scan", "rounds": rounds, "control": rounds == 0}
    for panel in args.report_panels:
        for name, executable, rounds in (("complete-report", "report", 1), ("report-preload", "compute", 0),
                                         ("repeated-report", "compute", args.report_rounds), ("live-checkout", "live", 1)):
            by_case[panel + "/" + name] = {"executable": executable, "rounds": rounds,
                                            "control": rounds == 0, "panel": panel}
    record = {"revision": cfg["revision"], "baseline_revision": cfg["baseline_revision"],
              "created_utc": datetime.now(timezone.utc).isoformat(), "completed": False,
              "config": cfg, "config_file": file_id(args.config.resolve()), "runner": file_id(Path(__file__).resolve()),
              "inputs": inputs, "panels": panels, "host": platform.uname()._asdict(),
              "clock": "perf_counter_ns around subprocess.run, including process creation, I/O and exit",
              "affinity_before": affinity, "cpu": args.cpu, "environment": env, "orders": plan,
              "samples": args.samples, "pilot": args.pilot, "cases": by_case, "capture_validation": {}}
    result_path = directory / "results.json"
    def save():
        temporary = directory / "results.tmp"
        temporary.write_text(json.dumps(record, indent=2) + "\n")
        temporary.replace(result_path)
    def run(command):
        start = time.perf_counter_ns()
        cp = subprocess.run(command, cwd=directory, env=env, stdin=subprocess.DEVNULL, capture_output=True, timeout=180)
        elapsed = time.perf_counter_ns() - start
        assert cp.returncode == 0 and not cp.stderr, (command, cp.returncode, cp.stderr[-2000:])
        return elapsed, cp.stdout
    for key, fixture in inputs["fixtures"].items():
        target = Path(fixture["capture_oracle"]["path"]).read_bytes()
        record["capture_validation"][key] = {}
        for engine, cmds in cfg["engines"].items():
            _, stdout = run([*cmds["scan"], fixture["input"]["path"], "1", "dump"])
            assert stdout == target, (key, engine, "capture mismatch")
            record["capture_validation"][key][engine] = sha(stdout)
    for name, case in by_case.items():
        output = directory / "report.out"
        def sample(engine):
            operation = case["executable"]
            rounds = case["rounds"]
            cmd = cfg["engines"][engine][operation]
            if operation == "scan":
                fixture = inputs["fixtures"]["corpus"]
                cmd = [*cmd, fixture["input"]["path"], str(rounds), "checksum"]
                expected = f"{fixture['matches']*rounds} {fixture['checksum']*rounds}\n".encode()
            else:
                panel = case["panel"]
                report = reports[panel]
                output.write_bytes(b"sentinel")
                if operation == "live":
                    cmd = [*cmd, *cfg["live_arguments"][panel]]
                else:
                    cmd = [*cmd, *full[panel].arguments()]
                if operation == "compute":
                    cmd += ["--rounds", str(rounds)]
                cmd += ["--output", str(output)]
                expected = f"{len(report)*rounds}\n".encode() if operation == "compute" else b""
            ns, stdout = run(cmd)
            assert stdout == expected, (name, engine, stdout[:120], expected[:120])
            observed = stdout if operation == "scan" else output.read_bytes()
            if operation != "scan":
                assert observed == (b"" if rounds == 0 else report.encode()), (name, engine, "report bytes differ")
            return {"engine": engine, "elapsed_ns": ns, "stdout_sha256": sha(stdout), "output_sha256": sha(observed)}
        case["warmup"] = [sample(e) for e in plan[0][::-1]]
        case["runs"] = []
        for i in range(args.samples):
            case["runs"].append([sample(e) for e in plan[i % len(plan)]])
            save()
        med = {e: statistics.median(s["elapsed_ns"] for row in case["runs"] for s in row if s["engine"] == e) for e in ENGINES}
        case.update(medians_ns=med, after_over_rust=med["after"]/med["rust"], after_over_before=med["after"]/med["before"])
        print(name, {e: round(x/1e6, 2) for e, x in med.items()}, round(case["after_over_rust"], 4), flush=True)
    pairs = [("scanner", "scanner-preload")] + [
        (panel + "/repeated-report", panel + "/report-preload") for panel in args.report_panels
    ]
    record["per_round_ns"] = {
        case: {engine: (by_case[case]["medians_ns"][engine] - by_case[base]["medians_ns"][engine]) / by_case[case]["rounds"]
               for engine in ENGINES} for case, base in pairs
    }
    gated = [c["after_over_rust"] for c in by_case.values() if not c["control"]]
    gated += [v["after"]/v["rust"] for v in record["per_round_ns"].values()]
    record["time_target_met"] = all(0 < ratio <= 2 for ratio in gated)
    assert all(file_id(Path(v["path"])) == v for v in cfg["guarded_inputs"])
    record["completed"] = True
    save()
    if not args.pilot and not record["time_target_met"]:
        raise SystemExit("2x Rust target missed; all observations retained")


if __name__ == "__main__":
    main()
