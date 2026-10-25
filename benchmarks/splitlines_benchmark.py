"""Compare materialized, streaming-owned and streaming-borrowed lines on uv and Ruff."""

import argparse
import hashlib
import itertools
import json
import os
from pathlib import Path
import random
import statistics
import subprocess
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
import catalog_validate
import conformance_program_benchmark as ruff
import stream_catalog_benchmark as catalog


ARMS = ("baseline", "owned", "borrowed")
CASES = tuple(
    (app, scale, kind)
    for app, kinds in (("catalog", ("plain", "gzip")), ("ruff", ("diff", "github")))
    for scale in (1, 10)
    for kind in kinds
)
ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "examples/conformance_report/upstream/conformance.py"


def sha(data):
    return hashlib.sha256(data).hexdigest()


def label(case):
    return "-".join(map(str, case))


def write(path, value):
    path.parent.mkdir(exist_ok=True, parents=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def prepare(args):
    inputs = args.work / "inputs"
    catalog.prepare(inputs / "catalog", args.python)
    panels = inputs / "panels"
    subprocess.run([sys.executable, "-B", ROOT / "benchmarks/conformance_grouping_inputs.py",
                    "--archive", ROOT / "benchmarks/results/2026-10-22-collector-ownership-study.tar.xz",
                    "--output", panels], check=True)
    meta = {}
    for scale in (1, 10):
        _, _, meta[str(scale)] = ruff.replay(panels, args.typing_conformance, scale, inputs / "ruff")
    write(inputs / "ruff/replay.json", meta)


def invoke(args, case, arm, out, prefix=(), audit=False):
    app, scale, kind = case
    if arm == "python":
        cmd = (catalog_validate.original_command(args.python) if app == "catalog"
               else [str(args.python), "-B", str(SOURCE)])
    else:
        # The native argv[0] is a Python string, so its length affects allocation counts.
        program = out / "native"
        if program.is_symlink():
            program.unlink()
        program.symlink_to(args.apps / arm / app)
        cmd = [str(program)]
    env = ruff.environment(args.work)
    env.update(CONFORMANCE_SUITE_COMMIT=ruff.REVISION, RAYON_NUM_THREADS="1")
    if prefix:
        env["TMPDIR"] = str(out)
    if "VALGRIND_LIB" in os.environ:
        env["VALGRIND_LIB"] = os.environ["VALGRIND_LIB"]
    if app == "catalog":
        cwd = args.work / "inputs/catalog"
        dest = out / "catalog.json"
        dest.write_bytes(b"before\n")
        cmd += [str(cwd / (f"{scale}x.ndjson" + (".gz" if kind == "gzip" else ""))), str(dest)]
    else:
        cwd = args.work / f"inputs/ruff/{scale}x"
        cmd += ["--tests-path", str(cwd), "--old-ty", str(cwd / "old-ty"),
                "--new-ty", str(cwd / "new-ty"), "--format", kind]
        if audit:
            # Every arm must issue both invocations afresh; old files must not satisfy the audit.
            for side in ("old", "new"):
                (cwd / f"{side}.argv").unlink(missing_ok=True)
            env["IMRE_BENCH_AUDIT"] = "1"
    started = time.perf_counter_ns()
    run = subprocess.run([*prefix, *cmd], cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                         capture_output=True, timeout=900)
    elapsed = time.perf_counter_ns() - started
    if run.returncode or run.stderr or (app == "catalog" and run.stdout):
        raise RuntimeError((cmd, run.returncode, run.stderr[:2000], run.stdout[:100]))
    data = dest.read_bytes() if app == "catalog" else run.stdout
    identity = {"bytes": len(data), "sha256": sha(data)}
    if audit and app == "ruff":
        for side in ("old", "new"):
            argv = (cwd / f"{side}.argv").read_bytes()
            assert argv.split(b"\0", 1)[0] == os.fsencode(cwd)
            identity[side + "_cwd_argv_sha256"] = sha(argv)
    return elapsed, identity


def oracles(args):
    out = args.work / "oracles"
    out.mkdir(exist_ok=True)
    result = {}
    for case in CASES:
        _, result[label(case)] = invoke(args, case, "python", out, audit=True)
        if case[0] == "catalog":
            ref = (args.work / "inputs/catalog/expected.json").read_bytes()
            assert result[label(case)]["sha256"] == sha(ref)
    write(out / "outputs.json", result)


def checked(args, case, arm, out, expected, *, prefix=(), audit=False):
    elapsed, got = invoke(args, case, arm, out, prefix, audit)
    keys = expected.keys() if audit else ("bytes", "sha256")
    if any(got.get(key) != expected[key] for key in keys):
        raise ValueError((label(case), arm, expected, got))
    return elapsed


def intervals(samples, seed):
    result = {}
    for left, right in itertools.combinations(ARMS, 2):
        ratios = [row[left] / row[right] for row in samples]
        rng = random.Random(seed)
        bootstrap = sorted(statistics.median(rng.choices(ratios, k=len(ratios))) for _ in range(4000))
        result[f"{left}/{right}"] = [statistics.median(ratios), bootstrap[100], bootstrap[3899]]
    return result


def measure(args, case, expected, out):
    for arm in ARMS:
        checked(args, case, arm, out, expected, audit=True)
    design = list(itertools.permutations(ARMS))
    orders = []
    rng = random.Random(args.seed)
    for _ in range(args.blocks // len(design)):
        rng.shuffle(design)
        orders += design
    def host_state():
        cpu = next(line for line in Path("/proc/stat").read_text().splitlines()
                   if line.startswith(f"cpu{args.cpu} "))
        return {"cpu_ticks": cpu, "loadavg": os.getloadavg()}

    before = host_state()
    rows = [{arm: checked(args, case, arm, out, expected) for arm in order} for order in orders]
    after = host_state()
    return {"cpu": args.cpu, "seed": args.seed, "orders": orders, "samples_ns": rows,
            "host_before": before, "host_after": after,
            "median_ms": {arm: statistics.median(row[arm] for row in rows) / 1e6 for arm in ARMS},
            "paired_speedup_95ci": intervals(rows, args.seed)}


def profile(args, case, expected, out):
    totals = {}
    for arm in args.arms:
        trace = out / f"{arm}.dhat.json"
        prefix = [str(args.valgrind), "--tool=dhat", "--mode=heap", "--trace-children=no",
                  f"--dhat-out-file={trace}", f"--log-file={out / (arm + '.log')}"]
        if args.debug_info:
            prefix.append(f"--extra-debuginfo-path={args.debug_info}")
        checked(args, case, arm, out, expected, prefix=prefix)
        data = json.loads(trace.read_bytes())
        # gb/gbk describe all allocation sites at the same global heap high-water mark.
        totals[arm] = {key: sum(p[key] for p in data["pps"]) for key in ("tb", "tbk", "gb", "gbk")}
        totals[arm]["trace_sha256"] = sha(trace.read_bytes())
    return totals


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("prepare", "oracles", "timing", "allocations"))
    parser.add_argument("--work", required=True, type=Path)
    parser.add_argument("--apps", type=Path, help="directory with baseline, owned and borrowed executables")
    parser.add_argument("--python", type=Path, default=Path(sys.executable))
    parser.add_argument("--typing-conformance", type=Path)
    parser.add_argument("--case", action="append", choices=[label(case) for case in CASES])
    parser.add_argument("--arms", choices=ARMS, nargs="+", default=ARMS)
    parser.add_argument("--blocks", type=int, default=36)
    parser.add_argument("--seed", type=int, default=31259)
    parser.add_argument("--cpu", type=int)
    parser.add_argument("--run", default="primary")
    parser.add_argument("--valgrind", type=Path, default=Path("/usr/bin/valgrind"))
    parser.add_argument("--debug-info", type=Path)
    args = parser.parse_args()
    args.work = args.work.resolve()
    args.apps = (args.apps or args.work / "apps").resolve()
    args.python = args.python.resolve(strict=True)
    if args.mode == "prepare":
        if not args.typing_conformance:
            parser.error("prepare needs the pinned typing checkout's conformance directory")
        prepare(args)
    elif args.mode == "oracles":
        oracles(args)
    else:
        if args.cpu is None or args.blocks < 6 or args.blocks % 6:
            parser.error("use --cpu and a positive multiple of six blocks")
        os.sched_setaffinity(0, {args.cpu})
        references = json.loads((args.work / "oracles/outputs.json").read_text())
        for case in CASES:
            key = label(case)
            if args.case and key not in args.case:
                continue
            out = args.work / args.mode / args.run / key
            out.mkdir(parents=True, exist_ok=True)
            result = (measure if args.mode == "timing" else profile)(args, case, references[key], out)
            write(out / "result.json", result)
            print(key, result.get("paired_speedup_95ci", result), flush=True)


if __name__ == "__main__":
    main()
