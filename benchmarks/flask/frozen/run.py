"""Build this checkout and run the frozen Flask / Rust comparison on Linux."""

import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
BENCH = HERE.parent
ROOT = BENCH.parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--smoke", action="store_true", help="run admission and one short paired trial"
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
    for program in ("cargo", "go", args.clang, "patch", "taskset", "strace", "readelf"):
        if shutil.which(program) is None:
            parser.error(f"{program} is required")
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
    cmd = [
        sys.executable,
        "-IB",
        HERE / "run_pair.py",
        prepared,
        "--client",
        client,
        "--rust",
        target / "release/imre-flask-hyper-control",
        "--server-cpu",
        str(server),
        "--client-cpus",
        clients,
        "--output",
        result,
    ]
    if args.smoke:
        cmd.extend(["--rounds", "1", "--warmup", "9", "--requests", "18"])
    subprocess.run(cmd, check=True)
    print(f"Results: {result}")


if __name__ == "__main__":
    main()
