"""Compare conformance grouping, equivalence, per-file metrics and complete reports."""

import argparse
import json
import os
import platform
import runpy
import statistics
import subprocess
import time
from hashlib import sha256
from pathlib import Path

shared = runpy.run_path(str(Path(__file__).with_name("conformance_regex_benchmark.py")))
ENGINES = shared["ENGINES"]
VALIDATION = shared["VALIDATION"]
file_id = shared["file_id"]
orders = shared["orders"]


def digest(data):
    return sha256(data).hexdigest()


def invocation(engine, mode, arguments, workload, output, rounds):
    if mode not in {"compute", "report"}:
        return [*engine["analysis"], mode, *arguments, str(rounds)], None
    args = [*workload.arguments(), "--output", str(output)]
    if mode == "compute":
        args.extend(["--rounds", str(rounds)])
    return [*engine[mode], *args], output


def summaries(cases):
    ratios = {}
    for name, case in cases.items():
        measurements = {}
        for engine in ENGINES:
            samples = [
                o for row in case["samples"] for o in row if o["engine"] == engine
            ]
            times = [
                (s["measure"]["ns"] - s["zero"]["ns"]) / case["rounds"]
                if "zero" in s
                else s["measure"]["ns"]
                for s in samples
            ]
            measurements[engine] = statistics.median(times)
        case["median_ns"] = measurements
        assert all(t > 0 for t in measurements.values()), (name, measurements)
        ratios[name] = measurements["after"] / measurements["rust"]
    return ratios


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--config", type=Path, required=True)
    ap.add_argument("--fixtures", type=Path, required=True)
    ap.add_argument("--output", type=Path, required=True)
    ap.add_argument("--cpu", type=int, required=True)
    ap.add_argument("--samples", type=int, default=12)
    ap.add_argument("--pilot", action="store_true")
    opts = ap.parse_args()
    cfg = json.loads(opts.config.read_bytes())
    engines = cfg["engines"]
    assert set(engines) == set(ENGINES) and all(
        set(v) == {"analysis", "report", "compute"} for v in engines.values()
    )
    assert (
        cfg["mode"] in ("normal", "thinlto")
        and len(cfg["revision"]) == len(cfg["baseline_revision"]) == 40
    )
    assert all(
        Path(command[0]).is_file() for e in engines.values() for command in e.values()
    )
    if not opts.pilot:
        assert (
            opts.samples >= 12
            and opts.samples % len(ENGINES) == 0
            and cfg["build_records"]
            and cfg["guarded_inputs"]
        )
        commands = {
            str(Path(c[0]).resolve()) for e in engines.values() for c in e.values()
        }
        guarded = {str(Path(g["path"]).resolve()) for g in cfg["guarded_inputs"]}
        assert commands <= guarded

    def verify_inputs():
        for f in cfg.get("guarded_inputs", []) + cfg.get("build_records", []):
            assert file_id(Path(f["path"])) == f, f

    verify_inputs()
    target = opts.output.resolve()
    target.mkdir(parents=True)
    fixtures = opts.fixtures.resolve()
    provenance = json.loads((fixtures / "provenance.json").read_bytes())
    for name, panel in provenance["panels"].items():
        for key, expected in panel["inputs"].items():
            assert digest((fixtures / name / (key + ".json")).read_bytes()) == expected
    os.sched_setaffinity(0, {opts.cpu})
    env = VALIDATION["environment"](target)
    plan = orders()
    cases = {}
    gates = {}
    verify = {}
    record = {
        "schema": 1,
        "config": cfg,
        "fixtures": provenance,
        "driver": file_id(Path(__file__).resolve()),
        "cpu": opts.cpu,
        "host": platform.uname()._asdict(),
        "order": plan,
        "samples_per_engine": opts.samples,
        "pilot": opts.pilot,
        "parity": verify,
        "cases": cases,
        "completed": False,
    }

    def save():
        tmp = target / "results.tmp"
        tmp.write_text(json.dumps(record, indent=2) + "\n")
        tmp.replace(target / "results.json")

    def run(command, output=None):
        if output:
            output.unlink(missing_ok=True)
        start = time.perf_counter_ns()
        cp = subprocess.run(
            command,
            env=env,
            cwd=target,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            timeout=180,
            check=False,
        )
        elapsed = time.perf_counter_ns() - start
        assert cp.returncode == 0 and not cp.stderr, (
            command,
            cp.returncode,
            cp.stderr[-2000:],
        )
        data = output.read_bytes() if output else cp.stdout
        return (
            cp.stdout,
            data,
            {"ns": elapsed, "stdout": digest(cp.stdout), "output": digest(data)},
        )

    def checked(command, expected, output=None):
        stdout, body, usage = run(command, output)
        assert (stdout, body) == expected, (
            command,
            usage,
            tuple(digest(b) for b in expected),
        )
        return usage

    out = target / "report.md"
    for scale in (1, 10):
        workloads = {}
        for state in ("changed", "unchanged"):
            p = fixtures / f"{scale}x" / state
            paths = [str(p / (role + ".json")) for role in ("manifest", "old", "new")]
            for fmt in ("github", "diff"):
                workloads[(state, fmt)] = VALIDATION["Workload"](
                    *(Path(s) for s in paths), fmt
                )
            for mode in ("dump", "aliases"):
                command = [mode, *paths, "1"]
                stdout, body, _ = run([*engines["upstream"]["analysis"], *command])
                key = f"{scale}x-{state}-{mode}"
                (target / (key + ".txt")).write_bytes(body)
                verify[key] = {
                    "oracle": file_id(target / (key + ".txt")),
                    "engines": {
                        engine: checked([*cfg["analysis"], *command], (stdout, body))[
                            "output"
                        ]
                        for engine, cfg in engines.items()
                    },
                }
        rounds_by_mode = (
            {"group": 32, "equivalent": 512, "files": 16}
            if scale == 1
            else {"group": 8, "equivalent": 64, "files": 4}
        )
        specs = [
            (mode, "changed", "github", rounds)
            for mode, rounds in rounds_by_mode.items()
        ]
        specs += [("report", state, fmt, 1) for state, fmt in workloads]
        specs.append(("compute", "changed", "github", 5 if scale == 1 else 2))
        for mode, state, fmt, rounds in specs:
            path = fixtures / f"{scale}x" / state
            arguments = [
                str(path / (role + ".json")) for role in ("manifest", "old", "new")
            ]
            workload = workloads[state, fmt]
            operation = mode if mode in {"report", "compute"} else "analysis"

            name = f"{scale}x/{state}-{fmt}/{mode}"
            paired = mode != "report"
            ns = (0, rounds) if paired else (rounds,)
            oracle = {}
            for n in ns:
                cmd, output = invocation(
                    engines["upstream"], mode, arguments, workload, out, n
                )
                stdout, full, _ = run(cmd, output)
                if operation != "analysis":
                    expected_report = (
                        VALIDATION["oracle"](workload).report.encode() if n else b""
                    )
                    assert full == expected_report
                    if n:
                        (target / f"{scale}x-{state}-{fmt}.md").write_bytes(full)
                oracle[n] = stdout, full
            case = cases[name] = {
                "rounds": rounds,
                "paired": paired,
                "oracle": {
                    str(n): [digest(b) for b in value] for n, value in oracle.items()
                },
                "samples": [],
            }
            for sample in range(-1, opts.samples):
                order = plan[sample % len(plan)]
                row = []
                for pos, engine in enumerate(order):
                    s = {"engine": engine}
                    sequence = ns if (sample + pos) % 2 == 0 else tuple(reversed(ns))
                    s["execution_order"] = list(sequence)
                    for n in sequence:
                        cmd, output = invocation(
                            engines[engine], mode, arguments, workload, out, n
                        )
                        s["measure" if n else "zero"] = checked(cmd, oracle[n], output)
                    row.append(s)
                if sample == -1:
                    case["warmup"] = row
                else:
                    case["samples"].append(row)
                save()
            gates.update(summaries({name: case}))
            print(
                name,
                {e: round(v / 1e6, 3) for e, v in case["median_ns"].items()},
                "Relar/Rust",
                round(gates[name], 3),
                flush=True,
            )
    verify_inputs()
    record.update(
        completed=True,
        ratios=gates,
        time_target_met=all(v <= 2 for v in gates.values()),
    )
    save()
    if not opts.pilot and not record["time_target_met"]:
        raise SystemExit("2x Rust target missed; measurements retained")


if __name__ == "__main__":
    main()
