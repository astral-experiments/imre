"""Compare unchanged Base45 in balanced Relar/CPython/Rust process batches."""

import argparse
import base64
import itertools
import json
import platform
import random
import runpy
import subprocess
import time
from datetime import datetime, timezone
from pathlib import Path
from types import SimpleNamespace

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
REFERENCE = HERE / "reference/base45_codec"
OPERATIONS = ("encode", "decode-bytes", "decode-text")
shared = SimpleNamespace(**runpy.run_path(str(HERE / "benchmark.py")))
validator = SimpleNamespace(**runpy.run_path(str(HERE / "base45_validate.py")))


def run_checked(command: list[str], timeout: float = 120) -> str:
    result = subprocess.run(
        command, input=b"", capture_output=True, timeout=timeout, check=False
    )
    if result.returncode or result.stderr:
        raise RuntimeError((command, result.returncode, result.stdout, result.stderr))
    return result.stdout.decode()


def workloads(codec, batch_bytes: int, operations=OPERATIONS) -> list[dict]:
    generator = random.Random(9285)
    result = []
    for size in (64, 1024, 16384):
        raw = generator.randbytes(size)
        encoded = codec.b45encode(raw)
        repetitions = max(1, batch_bytes // size)
        for operation in operations:
            payload = raw if operation == "encode" else encoded
            expected = validator.oracle(codec, operation, payload, repetitions)
            result.append(
                {
                    "label": f"{operation}-{size}",
                    "operation": operation,
                    "raw_bytes": size,
                    "repetitions": repetitions,
                    "input_base64": base64.b64encode(payload).decode(),
                    "expected_stdout": expected["stdout"],
                    "expected_output_sha256": validator.digest(expected["output"]),
                    "expected_output_bytes": len(expected["output"]),
                }
            )
    return result


def measure(workload: dict, commands: dict, directory: Path, args) -> dict:
    payload = base64.b64decode(workload["input_base64"])
    directories = {name: directory / name for name in commands}
    for path in directories.values():
        path.mkdir(parents=True)
        (path / "input.bin").write_bytes(payload)

    def run(name: str) -> dict:
        path = directories[name]
        output = path / "output.bin"
        output.write_bytes(b"unchanged output")
        command = [
            *commands[name],
            workload["operation"],
            str(path / "input.bin"),
            str(output),
            str(workload["repetitions"]),
        ]
        start = time.perf_counter_ns()
        process = subprocess.run(
            command,
            input=b"",
            capture_output=True,
            cwd=path,
            timeout=args.timeout,
            check=False,
        )
        elapsed = time.perf_counter_ns() - start
        actual = output.read_bytes()
        assert process.returncode == 0 and not process.stderr, (name, process)
        assert process.stdout.decode() == workload["expected_stdout"], (
            name,
            process.stdout,
        )
        assert validator.digest(actual) == workload["expected_output_sha256"]
        assert len(actual) == workload["expected_output_bytes"]
        assert (path / "input.bin").read_bytes() == payload
        return {
            "elapsed_ns": elapsed,
            "stdout": process.stdout.decode(),
            "stderr": "",
            "returncode": 0,
            "output_sha256": validator.digest(actual),
        }

    gates = {name: run(name) for name in commands}
    phases = {}
    orders = tuple(itertools.permutations(commands))
    for phase, count in (("warmups", args.warmups), ("samples", args.samples)):
        samples = []
        for index in range(count):
            order = orders[index % len(orders)]
            sample = {"index": index, "order": order}
            for name in order:
                sample[name] = run(name)
            samples.append(sample)
        phases[phase] = samples
    summaries = {name: shared.summary(phases["samples"], name) for name in commands}
    ratios = {
        f"{left}_over_{right}": summaries[left]["median_seconds"]
        / summaries[right]["median_seconds"]
        for left, right in (("cpython", "relar"), ("relar", "rust"), ("cpython", "rust"))
        if left in summaries and right in summaries
    }
    return {
        **workload,
        "gates": gates,
        **phases,
        "summary": summaries,
        **ratios,
    }


def rust_sources(directory: Path) -> dict[str, Path]:
    return {
        str(path.relative_to(directory)): path
        for path in (
            directory / "Cargo.toml",
            directory / "Cargo.lock",
            *sorted((directory / "src").rglob("*.rs")),
        )
    }


def rust_provenance(
    record_path: Path, paths: dict[str, Path], binary: Path, build_script: Path
) -> dict:
    record = json.loads(record_path.read_text())
    hashes = {name: shared.sha256(path) for name, path in paths.items()}
    contents = record.get("source_contents", {})
    assert record.get("entrypoint") == "base45"
    assert record.get("sources") == hashes and set(contents) == set(hashes)
    assert all(
        validator.digest(contents[name].encode()) == value
        for name, value in hashes.items()
    )
    assert record.get("binaries", {}).get("reference", {}).get(
        "sha256"
    ) == shared.sha256(binary)
    assert record.get("build_script_sha256") == shared.sha256(build_script)
    assert validator.digest(record.get("build_script", "").encode()) == shared.sha256(
        build_script
    )
    assert record.get("rustc_verbose") and record.get("cargo_version")
    return record


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--python", default="python3")
    parser.add_argument("--rust", type=Path, help="prebuilt independent Rust reference")
    parser.add_argument(
        "--rust-build-record",
        type=Path,
        help="defaults to build-record.json beside --rust",
    )
    parser.add_argument("--rust-source-root", type=Path, default=REFERENCE)
    parser.add_argument(
        "--native-only",
        action="store_true",
        help="measure Relar/Rust only; still validate against CPython",
    )
    parser.add_argument(
        "--operations",
        nargs="+",
        choices=OPERATIONS,
        default=OPERATIONS,
        help="codec operations to measure (default: all three)",
    )
    parser.add_argument("--clang", default="clang-18")
    parser.add_argument("--target")
    parser.add_argument("--lto-linker", type=Path)
    parser.add_argument("--artifact-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--samples", type=shared.positive_int, default=24)
    parser.add_argument("--warmups", type=shared.positive_int, default=6)
    parser.add_argument(
        "--batch-bytes", type=shared.positive_int, default=2 * 1024 * 1024
    )
    parser.add_argument("--timeout", type=shared.positive_float, default=60)
    parser.add_argument("--cpu", type=int)
    parser.add_argument("--allow-dirty", action="store_true")
    args = parser.parse_args()
    if args.native_only and not args.rust:
        parser.error("--native-only requires --rust")
    if args.rust_build_record and not args.rust:
        parser.error("--rust-build-record requires --rust")
    if len(set(args.operations)) != len(args.operations):
        parser.error("--operations must not repeat an operation")
    permutations = 6 if args.rust and not args.native_only else 2
    if (
        args.samples < max(4, permutations)
        or args.samples % permutations
        or args.warmups % permutations
    ):
        parser.error(
            f"sample count must be >= {max(4, permutations)} and samples/warmups must be multiples of {permutations}"
        )
    provenance = validator.verify_upstream()
    cpu = shared.pin_cpu(args.cpu)
    compiler = shared.executable(args.compiler)
    python = shared.executable(args.python)
    clang = shared.executable(args.clang)
    revision = run_checked(["git", "-C", str(ROOT), "rev-parse", "HEAD"]).strip()
    dirty = bool(run_checked(["git", "-C", str(ROOT), "status", "--porcelain"]).strip())
    if dirty and not args.allow_dirty:
        raise RuntimeError(
            "commit measured sources first, or use --allow-dirty for an exploratory run"
        )
    artifacts = args.artifact_dir.resolve()
    artifacts.mkdir(parents=True, exist_ok=True)
    binary = artifacts / "base45"
    llvm = artifacts / "base45.ll"
    validation_path = artifacts / "validation.json"
    inputs = {
        "compiler": Path(compiler),
        "runtime": args.runtime.resolve(),
        "python": Path(python),
        "clang": Path(clang),
        "codec": validator.PACKAGE / "base45/__init__.py",
        "driver": validator.ENTRY,
        "harness": Path(__file__).resolve(),
        "validator": HERE / "base45_validate.py",
        "shared_harness": HERE / "benchmark.py",
    }
    rust = Path(shared.executable(str(args.rust))) if args.rust else None
    rust_record = None
    if rust:
        source_root = args.rust_source_root.resolve(strict=True)
        sources = rust_sources(source_root)
        tracked_sources = rust_sources(REFERENCE)
        assert {name: shared.sha256(path) for name, path in sources.items()} == {
            name: shared.sha256(path) for name, path in tracked_sources.items()
        }, "Rust source snapshot differs from the tracked reference"
        assert shared.sha256(source_root / "build.py") == shared.sha256(
            REFERENCE / "build.py"
        )
        record_path = (
            args.rust_build_record or rust.parent / "build-record.json"
        ).resolve(strict=True)
        rust_record = rust_provenance(
            record_path, sources, rust, source_root / "build.py"
        )
        inputs.update(
            {
                "rust": rust,
                "rust_build_record": record_path,
                "rust_build_script": source_root / "build.py",
            }
        )
        inputs.update({f"rust_source_{name}": path for name, path in sources.items()})
        inputs.update(
            {
                f"tracked_rust_source_{name}": path
                for name, path in tracked_sources.items()
            }
        )
        inputs["tracked_rust_build_script"] = REFERENCE / "build.py"
    if args.lto_linker:
        inputs["lto_linker"] = args.lto_linker.absolute()
    inputs["upstream_manifest"] = validator.PACKAGE / "UPSTREAM.json"
    for index, name in enumerate(provenance["files"]):
        inputs[f"upstream_{index}"] = validator.PACKAGE / name
    identities = {path.resolve() for path in inputs.values()}
    outputs = {
        binary.resolve(),
        llvm.resolve(),
        validation_path.resolve(),
        args.output.resolve(),
    }
    if outputs & identities or len(outputs) != 4:
        raise ValueError("output paths must not overwrite sources or build tools")
    hashes = {name: shared.sha256(path) for name, path in inputs.items()}
    command = [
        compiler,
        "build",
        str(validator.ENTRY),
        "--module-root",
        str(validator.PACKAGE),
        "-o",
        str(binary),
        "--runtime",
        str(args.runtime.resolve()),
        "--clang",
        clang,
        "--opt-level",
        "3",
        "--emit-llvm",
        str(llvm),
    ]
    if args.target:
        command += ["--target", args.target]
    if args.lto_linker:
        command += ["--lto-linker", str(args.lto_linker.absolute())]
    start = time.perf_counter_ns()
    run_checked(command)
    build_ns = time.perf_counter_ns() - start
    binary_hash = shared.sha256(binary)
    validation_command = [
        python,
        "-I",
        "-B",
        str(HERE / "base45_validate.py"),
        "--binary",
        str(binary),
        "--compiler",
        compiler,
        "--output",
        str(validation_path),
    ]
    if rust:
        validation_command += ["--rust", str(rust)]
    run_checked(validation_command, timeout=240)
    validation = json.loads(validation_path.read_text())
    assert validation["binary_sha256"] == binary_hash == shared.sha256(binary)
    assert validation["compiler_sha256"] == hashes["compiler"]
    if rust:
        assert validation["rust"]["binary_sha256"] == hashes["rust"]
    python_info = json.loads(run_checked([python, "-I", "-c", shared.PYTHON_INFO]))
    assert python_info["implementation"] == "cpython"
    python_info.pop("executable")
    codec, _ = validator.load_upstream_tests()
    commands = {"relar": [str(binary)]}
    if not args.native_only:
        commands["cpython"] = validator.python_command(python)
    if rust:
        commands["rust"] = [str(rust)]
    record = {
        "schema": 2,
        "at": datetime.now(timezone.utc).isoformat(),
        "revision": revision,
        "dirty": dirty,
        "upstream": provenance,
        "platform": platform.platform(),
        "cpu": cpu,
        "python": python_info,
        "compiler_version": run_checked([compiler, "--version"]).strip(),
        "clang_version": run_checked([clang, "--version"]).strip(),
        "source_and_tool_sha256": hashes,
        "binary_sha256": binary_hash,
        "llvm_sha256": shared.sha256(llvm),
        "build_ns": build_ns,
        "compile_command": command,
        "requested_target": args.target,
        "llvm_target": next(
            (
                line
                for line in llvm.read_text().splitlines()
                if line.startswith("target triple")
            ),
            None,
        ),
        "validation": validation,
        "rust_build_record": rust_record,
        "measured_engines": list(commands),
        "operations": list(args.operations),
        "batch_bytes": args.batch_bytes,
        "engine_orders": list(itertools.permutations(commands)),
        "thinlto": args.lto_linker is not None,
        "samples": args.samples,
        "warmups": args.warmups,
        "method": "Whole-process wall time including startup, one input read and one output write; identical repeated codec calls and checksum in each engine; Rust black_box prevents repeated-work elimination; preparation and output verification excluded; all engine permutations balanced across samples and warmups.",
        "workloads": [],
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    for workload in workloads(codec, args.batch_bytes, args.operations):
        result = measure(workload, commands, artifacts / workload["label"], args)
        record["workloads"].append(result)
        args.output.write_text(json.dumps(record, indent=2) + "\n")
        ratios = ", ".join(
            f"{name}={value:.3f}x" for name, value in result.items() if "_over_" in name
        )
        print(f"{result['label']}: {ratios}", flush=True)
    assert hashes == {name: shared.sha256(path) for name, path in inputs.items()}
    assert record["binary_sha256"] == shared.sha256(binary)
    assert validator.verify_upstream() == provenance
    record["input_guards_passed"] = True
    args.output.write_text(json.dumps(record, indent=2) + "\n")


if __name__ == "__main__":
    main()
