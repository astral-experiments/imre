"""Time conformance reports and check every output against pinned Ruff."""

import argparse
import gzip
import json
import os
import platform
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from conformance_regex_benchmark import REVISION, file_id, orders
from conformance_validate import APP, Workload, digest, environment, oracle, original_command, workload_metadata


def measure(commands, args, expected, cpus, output, live):
    os.sched_setaffinity(0, cpus)
    env = environment(output.parent)
    if live:
        env["RAYON_NUM_THREADS"] = str(len(cpus))
    order = orders(tuple(commands))
    samples = []
    for row in [list(commands), *order]:
        times = {}
        for engine in row:
            output.unlink(missing_ok=True)
            start = time.perf_counter_ns()
            process = subprocess.run([*commands[engine], *args, "--output", str(output)],
                                     stdin=subprocess.DEVNULL, capture_output=True,
                                     env=env, cwd=output.parent, timeout=180)
            times[engine] = time.perf_counter_ns() - start
            if process.returncode or process.stdout or process.stderr:
                raise RuntimeError((engine, process.returncode, process.stdout, process.stderr))
            if output.read_bytes() != expected:
                raise ValueError(f"{engine} wrote different report bytes")
        samples.append(times)
    medians = {engine: statistics.median(row[engine] for row in samples[1:]) / 1e6 for engine in commands}
    return {
        "commands": commands, "arguments": args, "cpus": sorted(cpus), "environment": env,
        "bytes": len(expected), "sha256": digest(expected), "warmup_ns": samples[0],
        "order": order, "samples_ns": samples[1:], "median_ms": medians,
        "relar_over_rust": medians["relar"] / medians["rust"],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for flag in ("fixtures", "python", "previous", "relar", "codon", "rust", "output"):
        parser.add_argument("--" + flag, type=Path, required=True)
    parser.add_argument("--cpu", type=int, required=True)
    parser.add_argument("--full-matrix", action="store_true",
                        help="also time unchanged captures and both formats on the live checkout")
    parser.add_argument("--compact", action="store_true",
                        help="also time the unforced unchanged summary in both formats, frozen and live")
    parser.add_argument("--live", nargs=3, type=Path, metavar=("SUITE", "OLD_TY", "NEW_TY"))
    parser.add_argument("--live-cpus", type=int, nargs="+")
    args = parser.parse_args()
    if args.compact and not args.full_matrix:
        parser.error("--compact requires --full-matrix")
    if bool(args.live) != bool(args.live_cpus):
        parser.error("--live and --live-cpus must be provided together")
    provenance = json.loads((args.fixtures / "provenance.json").read_bytes())
    if provenance["revision"] != REVISION:
        raise ValueError("unexpected typing-suite revision")
    python = args.python.absolute()  # Preserve a venv's interpreter symlink.
    commands = {"cpython": original_command(python)}
    commands.update({name: [str(getattr(args, name).resolve(strict=True))]
                     for name in ("codon", "previous", "relar", "rust")})
    files = [Path(command[0]) for command in commands.values()]
    files.extend([Path(__file__), APP / "original.py", APP / "upstream/conformance.py"])
    if args.live:
        suite, old_ty, new_ty = (path.resolve(strict=True) for path in args.live)
        if subprocess.check_output(["git", "-C", str(suite), "rev-parse", "HEAD"], text=True).strip() != REVISION:
            raise ValueError("unexpected live checkout revision")
        files.extend([old_ty, new_ty, APP / "original_live.py"])
    artifacts = [file_id(p) for p in files]
    cases, inputs = {}, {}
    with tempfile.TemporaryDirectory(prefix="relar-render-cases-") as tmp:
        output = Path(tmp) / "report.md"
        for scale in (1, 10):
            for state in (("changed", "unchanged") if args.full_matrix else ("changed",)):
                panel_name = f"{scale}x/{state}"
                panel = args.fixtures.resolve() / panel_name
                paths = [panel / (role + ".json") for role in ("manifest", "old", "new")]
                if {p.stem: digest(p.read_bytes()) for p in paths} != provenance["panels"][panel_name]["inputs"]:
                    raise ValueError(f"{panel_name} capture differs from its manifest")
                inputs[panel_name] = workload_metadata(Workload(*paths))
                forces = (True, False) if args.compact and state == "unchanged" else (True,)
                for force in forces:
                    for fmt in ("github", "diff"):
                        workload = Workload(*paths, fmt, force)
                        expected = oracle(workload).report.encode()
                        name = f"{panel_name}-{fmt}" + ("" if force else "-compact")
                        cases[name] = measure(commands, workload.arguments(), expected, {args.cpu}, output, live=False)
                        print(name, cases[name]["median_ms"], flush=True)
        if args.live:
            live_commands = {**commands, "cpython": [str(python), "-E", "-s", "-B", str(APP / "original_live.py")]}
            for force in ((True, False) if args.compact else (True,)):
                for fmt in (("github", "diff") if args.full_matrix else ("github",)):
                    argv = ["--suite", str(suite), "--old-ty", str(old_ty if force else new_ty),
                            "--new-ty", str(new_ty), "--timeout", "30", "--format", fmt]
                    if force:
                        argv.append("--force-summary-table")
                    # The live diff includes checkout paths that differ from the captured fixtures.
                    subprocess.run([*live_commands["cpython"], *argv, "--output", str(output)],
                                   cwd=output.parent, env=environment(output.parent), capture_output=True,
                                   timeout=180, check=True)
                    case = f"live/{fmt}" + ("" if force else "-compact")
                    cases[case] = measure(live_commands, argv, output.read_bytes(), set(args.live_cpus), output, live=True)
                    print(case, cases[case]["median_ms"], flush=True)
    if [file_id(p) for p in files] != artifacts:
        raise ValueError("benchmark binaries or oracle changed during measurement")
    record = {"schema": 1, "invocation": sys.argv, "host": platform.uname()._asdict(),
              "artifacts": artifacts, "fixtures": inputs, "suite_revision": REVISION, "cases": cases}
    args.output.write_bytes(gzip.compress(json.dumps(record, indent=2).encode(), mtime=0))
    if any(case["relar_over_rust"] > 2 for case in cases.values()):
        raise SystemExit("a report exceeded 2x Rust")


if __name__ == "__main__":
    main()
