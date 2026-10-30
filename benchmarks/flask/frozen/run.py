"""Build this checkout and benchmark frozen Flask against Rust or all five servers."""

import argparse
from collections import Counter
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
BENCH = HERE.parent
ROOT = BENCH.parents[1]


def run_five_servers(prepared, common, output, smoke):
    rounds, warmup, requests = (1, 9, 18) if smoke else (3, 180, 1800)
    subprocess.run(
        [
            sys.executable,
            "-IB",
            BENCH / "comparison/compare.py",
            "--imre-binary",
            prepared / "mixed/server-O3",
            "--wheelhouse",
            prepared / "wheels",
            "--node",
            shutil.which("node"),
            "--bun",
            shutil.which("bun"),
            "--mixed-only",
            "--concurrency",
            "8",
            "--rounds",
            str(rounds),
            "--warmup",
            str(warmup),
            "--requests",
            str(requests),
            "--output",
            output,
            *common,
        ],
        env={**os.environ, "FLASK_SECRET_KEY": "local-probe-key"},
        check=True,
    )
    # The general harness can complete the other controls if native admission fails.
    report = json.loads(output.read_bytes())
    build = json.loads((prepared / "build.json").read_bytes())
    fingerprint = build["graphs"]["mixed"]["builds"]["3"]["elf_sha256"]
    if (
        report["imre_native_preflight"]["verified_native_responses"] != 10
        or report["imre_build"]["binary_sha256"] != fingerprint
    ):
        raise RuntimeError(
            "the built frozen O3 executable did not pass native admission"
        )
    names = ("imre", "cpython", "node", "bun", "rust")
    expected = Counter((name, n) for name in names for n in range(1, rounds + 1))
    trials = report["trials"]
    if Counter((row["server"], row["round"]) for row in trials) != expected:
        raise RuntimeError("expected one timed trial per server per round")
    for row in trials:
        if row["Requests"] != requests or row["All"]["Count"] != requests:
            raise RuntimeError(
                f"incomplete response validation for {row['server']} round {row['round']}"
            )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--smoke",
        action="store_true",
        help="run admission and one short trial per server",
    )
    parser.add_argument(
        "--five-servers",
        action="store_true",
        help="compare frozen Imre, stock CPython Flask, Node, Bun and Rust",
    )
    parser.add_argument(
        "--work",
        type=Path,
        help="new output directory (defaults to a temporary directory)",
    )
    parser.add_argument(
        "--wheelhouse", type=Path, help="read the seven pinned wheels offline"
    )
    parser.add_argument(
        "--server-cpu", type=int, help="defaults to the first available CPU"
    )
    parser.add_argument(
        "--client-cpus",
        help="comma-separated CPUs, default up to three other available CPUs",
    )
    parser.add_argument("--clang", default="clang-18")
    args = parser.parse_args()
    if (
        sys.platform != "linux"
        or sys.implementation.name != "cpython"
        or sys.version_info[:3] != (3, 14, 5)
        or sys.flags.optimize
    ):
        parser.error("run on Linux with CPython 3.14.5 and assertions enabled")
    programs = ["cargo", "go", args.clang, "patch", "taskset", "strace", "readelf"]
    if args.five_servers:
        programs.extend(["node", "bun"])
    for program in programs:
        if shutil.which(program) is None:
            parser.error(f"{program} is required")
    if args.five_servers:
        # The JSON control must recover the digits before its Number rounds them.
        probe = (
            "const digits = JSON.parse('9007199254740993', (_, v, c) => c?.source);"
            "process.exit(digits === '9007199254740993' ? 0 : 1)"
        )
        for js in ("node", "bun"):
            if subprocess.run([js, "-e", probe]).returncode:
                parser.error(
                    f"{js} needs JSON.parse reviver context.source for exact integers"
                )
    available = sorted(os.sched_getaffinity(0))
    server = available[0] if args.server_cpu is None else args.server_cpu
    other_cpus = [str(n) for n in available if n != server]
    clients = args.client_cpus or ",".join(other_cpus[:3])
    client_ids = {int(n) for n in clients.split(",") if n}
    if (
        not client_ids
        or server in client_ids
        or not ({server} | client_ids) <= set(available)
    ):
        parser.error("select distinct available CPUs for the server and client")
    if args.work is None:
        work = Path(tempfile.mkdtemp(prefix="imre-flask-frozen-"))
    else:
        work = args.work.resolve()
        work.mkdir(parents=True)
    print(f"Benchmark files: {work}", flush=True)
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")).resolve()
    cargo = ["cargo", "build", "--release", "--locked", "--target-dir", target]
    subprocess.run(
        [*cargo, "-p", "imre-compiler", "-p", "imre-runtime"], cwd=ROOT, check=True
    )
    subprocess.run(
        [*cargo, "--manifest-path", BENCH / "comparison/rust/Cargo.toml"],
        cwd=ROOT,
        check=True,
    )
    client = work / "client"
    subprocess.run(
        ["go", "build", "-o", client, BENCH / "comparison/client.go"],
        cwd=ROOT,
        check=True,
    )
    prepared = work / "build"
    prepare = [
        sys.executable,
        "-IB",
        HERE / "prepare.py",
        prepared,
        "--imre",
        target / "release/imre",
        "--runtime",
        target / "release/libimre_runtime.a",
        "--clang",
        args.clang,
    ]
    if args.wheelhouse is not None:
        prepare.extend(["--wheelhouse", args.wheelhouse.resolve(strict=True)])
    subprocess.run(prepare, check=True)
    result = work / "results.json"
    common = [
        "--client",
        client,
        "--rust",
        target / "release/imre-flask-hyper-control",
        "--server-cpu",
        str(server),
        "--client-cpus",
        clients,
    ]
    if args.five_servers:
        run_five_servers(prepared, common, result, args.smoke)
    else:
        cmd = [
            sys.executable,
            "-IB",
            HERE / "run_pair.py",
            prepared,
            "--output",
            result,
            *common,
        ]
        if args.smoke:
            cmd.extend(["--rounds", "1", "--warmup", "9", "--requests", "18"])
        subprocess.run(cmd, check=True)
    print(f"Results: {result}")


if __name__ == "__main__":
    main()
