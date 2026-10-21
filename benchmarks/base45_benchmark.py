"""Compare the unchanged Base45 codec in balanced native/CPython process batches."""

import argparse
import base64
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
shared = SimpleNamespace(**runpy.run_path(str(HERE / "benchmark.py")))
validator = SimpleNamespace(**runpy.run_path(str(HERE / "base45_validate.py")))


def run_checked(command: list[str], timeout: float = 120) -> str:
    result = subprocess.run(
        command, input=b"", capture_output=True, timeout=timeout, check=False
    )
    if result.returncode or result.stderr:
        raise RuntimeError((command, result.returncode, result.stdout, result.stderr))
    return result.stdout.decode()


def workloads(codec, batch_bytes: int) -> list[dict]:
    generator = random.Random(9285)
    result = []
    for size in (64, 1024, 16384):
        raw = generator.randbytes(size)
        encoded = codec.b45encode(raw)
        repetitions = max(1, batch_bytes // size)
        for operation in ("encode", "decode-bytes", "decode-text"):
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
    for phase, count in (("warmups", args.warmups), ("samples", args.samples)):
        samples = []
        for index in range(count):
            order = ("relar", "cpython") if index % 2 == 0 else ("cpython", "relar")
            sample = {"index": index, "order": order}
            for name in order:
                sample[name] = run(name)
            samples.append(sample)
        phases[phase] = samples
    summaries = {name: shared.summary(phases["samples"], name) for name in commands}
    speedup = (
        summaries["cpython"]["median_seconds"] / summaries["relar"]["median_seconds"]
    )
    return {
        **workload,
        "gates": gates,
        **phases,
        "summary": summaries,
        "cpython_over_relar": speedup,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--python", default="python3")
    parser.add_argument("--clang", default="clang-18")
    parser.add_argument("--target")
    parser.add_argument("--lto-linker", type=Path)
    parser.add_argument("--artifact-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--samples", type=shared.positive_int, default=20)
    parser.add_argument("--warmups", type=shared.positive_int, default=2)
    parser.add_argument(
        "--batch-bytes", type=shared.positive_int, default=2 * 1024 * 1024
    )
    parser.add_argument("--timeout", type=shared.positive_float, default=60)
    parser.add_argument("--cpu", type=int)
    parser.add_argument("--allow-dirty", action="store_true")
    args = parser.parse_args()
    if args.samples < 4 or args.samples % 2 or args.warmups % 2:
        parser.error("use an even sample count >= 4 and an even warmup count")
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
    run_checked(
        [
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
        ],
        timeout=180,
    )
    validation = json.loads(validation_path.read_text())
    assert validation["binary_sha256"] == binary_hash == shared.sha256(binary)
    assert validation["compiler_sha256"] == hashes["compiler"]
    python_info = json.loads(run_checked([python, "-I", "-c", shared.PYTHON_INFO]))
    assert python_info["implementation"] == "cpython"
    python_info.pop("executable")
    codec, _ = validator.load_upstream_tests()
    commands = {"relar": [str(binary)], "cpython": validator.python_command(python)}
    record = {
        "schema": 1,
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
        "thinlto": args.lto_linker is not None,
        "samples": args.samples,
        "warmups": args.warmups,
        "method": "Whole-process wall time including startup, one input read and one output write; identical repeated API calls and checksum in each engine; preparation and output verification excluded; balanced alternating order.",
        "workloads": [],
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    for workload in workloads(codec, args.batch_bytes):
        result = measure(workload, commands, artifacts / workload["label"], args)
        record["workloads"].append(result)
        args.output.write_text(json.dumps(record, indent=2) + "\n")
        print(
            f"{result['label']}: {result['cpython_over_relar']:.3f}x CPython/Relar",
            flush=True,
        )
    assert hashes == {name: shared.sha256(path) for name, path in inputs.items()}
    assert record["binary_sha256"] == shared.sha256(binary)
    assert validator.verify_upstream() == provenance
    record["input_guards_passed"] = True
    args.output.write_text(json.dumps(record, indent=2) + "\n")


if __name__ == "__main__":
    main()
