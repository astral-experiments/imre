"""Admit the frozen binary and time the identical nine-case workload against Rust/Hyper."""

import argparse
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
BENCH = HERE.parent
sys.path.insert(0, str(BENCH / "comparison"))
import compare as controls
import gate


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("work", type=Path)
    parser.add_argument("--rust", required=True, type=Path)
    parser.add_argument("--client", required=True, type=Path)
    parser.add_argument("--server-cpu", type=int, required=True)
    parser.add_argument("--client-cpus", required=True)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--rounds", type=int, default=5)
    parser.add_argument("--warmup", type=int, default=900)
    parser.add_argument("--requests", type=int, default=36000)
    args = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5) or sys.flags.optimize:
        parser.error("use CPython 3.14.5 with assertions enabled")
    if args.rounds < 1 or any(n < 9 or n % 9 for n in (args.requests, args.warmup)):
        parser.error(
            "use positive rounds and positive multiples of nine requests and warmups"
        )
    client_cpus = {int(cpu) for cpu in args.client_cpus.split(",")}
    if args.server_cpu in client_cpus or not (
        {args.server_cpu} | client_cpus
    ) <= os.sched_getaffinity(0):
        parser.error("provide distinct available server and client CPUs")
    client = args.client.resolve(strict=True)
    work = args.work.resolve(strict=True)
    rust = args.rust.resolve(strict=True)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    if args.output.exists():
        parser.error("output exists")
    os.environ["FLASK_SECRET_KEY"] = "local-probe-key"
    expected = json.loads((BENCH / "expected.json").read_bytes())
    cases = controls.cases()
    build = json.loads((work / "build.json").read_bytes())
    result = {
        "cpu": platform.processor(),
        "kernel": platform.release(),
        "server_cpu": args.server_cpu,
        "client_cpus": sorted(client_cpus),
        "concurrency": 8,
        "rounds": args.rounds,
        "warmup": args.warmup,
        "requests": args.requests,
        "client_sha256": gate.digest(client),
        "rust_sha256": gate.digest(rust),
        "native": {},
        "preflight": {},
        "trials": [],
    }

    def save():
        tmp = args.output.with_suffix(".partial")
        tmp.write_text(json.dumps(result, indent=2) + "\n")
        tmp.replace(args.output)

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        for opt in (0, 3):
            directory = root / f"admit-O{opt}"
            directory.mkdir()
            binary = work / "mixed" / f"server-O{opt}"
            fingerprint = build["graphs"]["mixed"]["builds"][str(opt)]["elf_sha256"]
            if gate.digest(binary) != fingerprint:
                raise ValueError(
                    f"admission binary does not match O{opt} build receipt"
                )
            row = gate.exercise([binary], directory, expected, prove_binary=binary)
            result["native"][str(opt)] = row
            save()
            if row["outcome"] != "pass":
                raise RuntimeError(
                    f"O{opt} native admission failed: {row.get('error') or row.get('mismatches')}"
                )
        fixtures = root / "cases.json"
        fixtures.write_text(json.dumps(cases))
        servers = {"frozen_imre": work / "mixed/server-O3", "rust": rust}
        for name, binary in servers.items():
            with controls.serve([binary], args.server_cpu) as (port, _, _):
                result["preflight"][name] = controls.preflight(
                    port, cases, expected, name == "frozen_imre"
                )
        env = {**os.environ, "GOMAXPROCS": str(len(client_cpus))}
        for round_no in range(args.rounds):
            order = list(servers) if round_no % 2 == 0 else list(reversed(servers))
            for name in order:
                with controls.serve(
                    [servers[name]], args.server_cpu, capture_errors=False
                ) as (port, proc, _):
                    if gate.digest(Path(f"/proc/{proc.pid}/exe")) != gate.digest(
                        servers[name]
                    ):
                        raise RuntimeError(f"timed executable differs for {name}")
                    command = [
                        "taskset",
                        "-c",
                        args.client_cpus,
                        client,
                        "-addr",
                        f"127.0.0.1:{port}",
                        "-cases",
                        fixtures,
                        "-profile",
                        "mixed",
                        "-c",
                        "8",
                    ]
                    subprocess.run(
                        [*command, "-n", str(args.warmup)],
                        check=True,
                        stdout=subprocess.DEVNULL,
                        env=env,
                    )
                    before = controls.process_cpu(proc.pid)
                    busy = controls.core_cpu(args.server_cpu)
                    sample = json.loads(
                        subprocess.check_output(
                            [*command, "-n", str(args.requests)], env=env
                        )
                    )
                    cpu = controls.process_cpu(proc.pid) - before
                    if sample["All"]["Count"] != args.requests or any(
                        sample["ByRoute"][route]["Count"] != args.requests // 3
                        for route in ("health", "hello", "total")
                    ):
                        raise ValueError(
                            "client did not validate the requested distribution"
                        )
                    sample.update(
                        server=name,
                        round=round_no + 1,
                        server_cpu_seconds=cpu,
                        other_server_core_seconds=max(
                            0.0, controls.core_cpu(args.server_cpu) - busy - cpu
                        ),
                    )
                    result["trials"].append(sample)
                    save()
                    print(
                        f"{round_no + 1} {name}: {sample['RPS']:.1f} requests/s",
                        flush=True,
                    )


if __name__ == "__main__":
    main()
