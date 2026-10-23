"""Compare complete Rust, CPython and Relar commands on a pinned Linux CPU."""

import argparse
import base64
import hashlib
import itertools
import json
import os
import platform
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path

# Keep the sibling harnesses importable when Python's safe-path mode is enabled.
sys.path.insert(0, str(Path(__file__).resolve().parent))

import base45_benchmark
import benchmark
import conformance_grouping_inputs as captures
import conformance_validate as conformance
import inventory_benchmark
import luhn_benchmark
import telemetry_benchmark

ROOT = Path(__file__).resolve().parents[1]
EXAMPLES = ROOT / "examples"
ENGINES = ("rust", "cpython", "relar")
WARMUPS = 2
ROUNDS = 12

# Each Rust binary implements the corresponding Python program's CLI.
PROGRAMS = {
    "inventory_reconciliation": ("__main__.py", "inventory-reference"),
    "base45_codec": ("driver.py", "base45-reference"),
    "luhn_checksum": ("driver.py", "luhn-reference"),
    "telemetry_comparison": ("__main__.py", "telemetry-reference"),
    "conformance_report": ("__main__.py", "conformance-report-reference"),
}


@dataclass
class Case:
    name: str
    size: str
    package: str
    directory: Path
    arguments: list[str] | tuple[str, ...]
    stdout: bytes
    report: bytes


def fixtures(work):
    cases = []
    inventory = inventory_benchmark.corpus()
    for shape, name, skus in (
        ("dense", "repeated", 64),
        ("sparse", "distinct", 16_384),
    ):
        value = next(v for v in inventory if v.label == f"{shape}-16384")
        directory = work / value.label
        directory.mkdir()
        validator = inventory_benchmark.validator
        validator.materialize(value, directory)
        expected = validator.oracle(value)
        cases.append(
            Case(
                f"Inventory ({name})",
                f"16,384 requests, {skus:,} SKUs",
                "inventory_reconciliation",
                directory,
                value.arguments,
                expected["stdout"].encode(),
                expected["files"]["report.txt"],
            )
        )

    base45, _ = base45_benchmark.validator.load_upstream_tests()
    luhn, _ = luhn_benchmark.validator.load_upstream()
    decode = next(
        w
        for w in base45_benchmark.workloads(base45, 2**20, ["decode-bytes"])
        if w["raw_bytes"] == 16_384
    )
    (checksum,) = luhn_benchmark.workloads(luhn, [16], 2**20, ["checksum"])
    for name, size, package, workload, codec, validator in [
        (
            "Base45 decode",
            "64 × 16 KiB decoded",
            "base45_codec",
            decode,
            base45,
            base45_benchmark.validator,
        ),
        (
            "Luhn checksum",
            "65,536 × 16 digits",
            "luhn_checksum",
            checksum,
            luhn,
            luhn_benchmark.validator,
        ),
    ]:
        directory = work / package
        directory.mkdir()
        payload = base64.b64decode(workload["input_base64"])
        (directory / "input.bin").write_bytes(payload)
        operation, repetitions = workload["operation"], workload["repetitions"]
        expected = validator.oracle(codec, operation, payload, repetitions)
        cases.append(
            Case(
                name,
                size,
                package,
                directory,
                [operation, "input.bin", "report.txt", str(repetitions)],
                expected["stdout"].encode(),
                expected["output"],
            )
        )

    (trace,) = telemetry_benchmark.workloads([4096], 1_000_000, ["report-mixed"])
    validator = telemetry_benchmark.validator
    value = validator.case_from_record(trace["case"])
    directory = work / "telemetry"
    directory.mkdir()
    validator.materialize(value, directory)
    expected = validator.oracle(value)
    cases.append(
        Case(
            "Telemetry report",
            f"{trace['repetitions']} × 4,096 aligned pairs",
            "telemetry_comparison",
            directory,
            value.arguments,
            expected["stdout"].encode(),
            expected["files"]["report.txt"],
        )
    )

    archive = ROOT / "benchmarks/results/2026-10-22-collector-ownership-study.tar.xz"
    changed, unchanged, sources = captures.unpack(archive, work / "capture")
    directory = work / "conformance"
    captures.prepare(changed, unchanged, directory, sources, scales=(1,))
    panel = directory / "1x/changed"
    report = conformance.Workload(
        *(panel / f"{name}.json" for name in ("manifest", "old", "new"))
    )
    expected = conformance.oracle(report)
    if expected.report is None:
        raise ValueError(
            f"Ruff could not render the reference report: {expected.error}"
        )
    cases.append(
        Case(
            "Conformance report",
            "146 files, two captured ty runs",
            "conformance_report",
            directory,
            [*report.arguments(), "--output", "report.txt"],
            b"",
            expected.report.encode(),
        )
    )
    return cases


def build(target, work, toolchain, clang):
    selected = ["+" + toolchain] if toolchain else []
    cargo = [
        "cargo",
        *selected,
        "build",
        "--release",
        "--locked",
        "--target-dir",
        target,
    ]
    env = {
        **{
            k: v
            for k, v in os.environ.items()
            if k
            not in (
                "RUSTFLAGS",
                "CARGO_ENCODED_RUSTFLAGS",
                "RELARFLAGS",
                "RELAR_ENCODED_FLAGS",
            )
        },
        "CARGO_PROFILE_RELEASE_OPT_LEVEL": "3",
        "CARGO_PROFILE_RELEASE_LTO": "off",
        "CARGO_PROFILE_RELEASE_CODEGEN_UNITS": "16",
        "XDG_CONFIG_HOME": str(work / "config"),
    }
    subprocess.run(
        [*cargo, "-p", "relar-compiler", "-p", "relar-runtime"],
        cwd=ROOT,
        env=env,
        check=True,
    )
    release = target / "release"
    compiler, runtime = release / "relar", release / "librelar_runtime.a"
    hashes = {
        "cpython": benchmark.sha256(Path(sys.executable)),
        "compiler": benchmark.sha256(compiler),
        "runtime": benchmark.sha256(runtime),
    }
    commands = {}
    for package, (entry, rust_name) in PROGRAMS.items():
        manifest = ROOT / "benchmarks/reference" / package / "Cargo.toml"
        subprocess.run(
            [*cargo, "--manifest-path", manifest], cwd=ROOT, env=env, check=True
        )
        module_root = EXAMPLES if entry == "__main__.py" else EXAMPLES / package
        source = EXAMPLES / package / entry
        binary = work / f"{package}-native"
        subprocess.run(
            [
                compiler,
                "build",
                source,
                "--module-root",
                module_root,
                "--runtime",
                runtime,
                "--clang",
                clang,
                "--opt-level",
                "3",
                "--no-cache",
                "-q",
                "-o",
                binary,
            ],
            cwd=ROOT,
            env=env,
            check=True,
        )
        python_args = ["-m", package] if entry == "__main__.py" else [str(source)]
        commands[package] = {
            "rust": [release / rust_name],
            "relar": [binary],
            "cpython": [sys.executable, "-s", *python_args],
        }
        hashes[package] = {
            "rust": benchmark.sha256(release / rust_name),
            "relar": benchmark.sha256(binary),
        }
    return commands, hashes


def measure(case, commands, env):
    report = case.directory / "report.txt"
    orders = list(itertools.permutations(ENGINES))
    rows = []
    for i in range(WARMUPS + ROUNDS):
        times = {}
        for engine in orders[(i - WARMUPS) % len(orders)]:
            report.unlink(missing_ok=True)
            start = time.perf_counter_ns()
            process = subprocess.run(
                [*commands[engine], *case.arguments],
                cwd=case.directory,
                env=env,
                stdin=subprocess.DEVNULL,
                capture_output=True,
                timeout=120,
                check=False,
            )
            times[engine] = time.perf_counter_ns() - start
            if process.returncode or process.stderr:
                raise RuntimeError(
                    f"{engine} failed on {case.name} ({process.returncode}): "
                    + process.stderr.decode(errors="replace")
                )
            if process.stdout != case.stdout or report.read_bytes() != case.report:
                raise ValueError(f"{engine} produced incorrect output for {case.name}")
        rows.append(times)
    return {
        "name": case.name,
        "input": case.size,
        "package": case.package,
        "stdout": case.stdout.decode(),
        "report_bytes": len(case.report),
        "report_sha256": hashlib.sha256(case.report).hexdigest(),
        "warmups_ns": rows[:WARMUPS],
        "samples_ns": rows[WARMUPS:],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--toolchain", help="Rust toolchain override (default: repository pin)"
    )
    parser.add_argument("--clang", default="clang-18")
    parser.add_argument("--cpu", type=int, help="default: first permitted CPU")
    parser.add_argument(
        "--target-dir",
        type=Path,
        default=Path(os.getenv("CARGO_TARGET_DIR", ROOT / "target")),
    )
    parser.add_argument(
        "--output", type=Path, default=ROOT / "benchmarks/local/runtime-suite.json"
    )
    args = parser.parse_args()
    if (
        sys.flags.optimize
        or sys.implementation.name != "cpython"
        or sys.version_info < (3, 14)
    ):
        parser.error("use CPython 3.14+ with assertions enabled")
    selected = ["+" + args.toolchain] if args.toolchain else []

    def version(*command):
        return subprocess.check_output(command, cwd=ROOT, text=True).strip()

    with tempfile.TemporaryDirectory(prefix="relar-suite-") as tmp:
        work = Path(tmp)
        commands, hashes = build(
            args.target_dir.resolve(), work, args.toolchain, args.clang
        )
        cases = fixtures(work)
        env = {
            "PATH": os.defpath,
            "LANG": "C.UTF-8",
            "HOME": tmp,
            "PYTHONPATH": os.pathsep.join(map(str, (EXAMPLES, ROOT / "stdlib"))),
            "PYTHONHASHSEED": "0",
            "PYTHON_JIT": "0",
        }
        python_info = json.loads(
            subprocess.check_output(
                [sys.executable, "-c", benchmark.PYTHON_INFO], env=env
            )
        )
        python_info.pop("executable")
        result = {
            "revision": version("git", "rev-parse", "HEAD"),
            "dirty": bool(version("git", "status", "--porcelain")),
            "date_utc": datetime.now(timezone.utc).date().isoformat(),
            "os": f"{platform.system()} {platform.release()} {platform.machine()}",
            "cpu": benchmark.pin_cpu(args.cpu),
            "python": python_info,
            "rustc": version("rustc", *selected, "-Vv"),
            "clang": version(args.clang, "--version").splitlines()[0],
            "relar": version(args.target_dir.resolve() / "release/relar", "--version"),
            "binaries_sha256": hashes,
            "cases": [],
        }
        for case in cases:
            print(f"Measuring {case.name}", flush=True)
            result["cases"].append(measure(case, commands[case.package], env))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
