"""Compare unchanged Luhn in balanced Relar/CPython/Rust process batches."""

import argparse
import base64
import itertools
import json
import math
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
REFERENCE = HERE / "reference/luhn_checksum"
shared = SimpleNamespace(**runpy.run_path(str(HERE / "benchmark.py")))
validator = SimpleNamespace(**runpy.run_path(str(HERE / "luhn_validate.py")))


def run_checked(command: list[str], timeout: float = 240) -> str:
    result = subprocess.run(
        command, input=b"", capture_output=True, timeout=timeout, check=False
    )
    if result.returncode or result.stderr:
        raise RuntimeError((command, result.returncode, result.stdout, result.stderr))
    return result.stdout.decode()


def baseline_provenance(record, hashes, upstream, rust_record, target, thinlto):
    """Bind a preserved binary to unchanged executable sources and build tools."""
    if (
        not record.get("input_guards_passed")
        or record.get("dirty") is not False
        or record.get("binary_sha256") != hashes["baseline"]
        or record.get("validation", {}).get("relar", {}).get("binary_sha256")
        != hashes["baseline"]
        or record.get("upstream") != upstream
    ):
        raise ValueError("baseline record must identify a validated, committed binary")
    if record.get("thinlto") != thinlto or record.get("requested_target") != target:
        raise ValueError("baseline target and linking mode must match")
    command = record["compile_command"]
    if (
        "--opt-level" not in command
        or command[command.index("--opt-level") + 1] != "3"
        or ("--lto-linker" in command) != thinlto
    ):
        raise ValueError("baseline compilation must use matching O3/linking options")
    original = record["source_and_tool_sha256"]
    matched_tools = [
        "python",
        "clang",
        "validator",
        "shared_harness",
        "shared_validator",
    ]
    if thinlto:
        matched_tools.append("lto_linker")
    if any(original.get(name) != hashes[name] for name in matched_tools):
        raise ValueError("baseline protocol, validator, or benchmark tool changed")
    old_rust = record["rust_build_record"]
    if not old_rust.get("input_guards_passed") or any(
        old_rust.get(name) != rust_record[name]
        for name in ("sources", "tool_sha256", "thinlto", "llvm_budget")
    ):
        raise ValueError("baseline Rust sources, toolchain, or LLVM budgets changed")
    source_keys = {
        name
        for name in original.keys() | hashes.keys()
        if name.startswith(("example:", "rust_source:"))
    }
    differences = []
    for name in sorted(source_keys):
        before, after = original.get(name), hashes.get(name)
        if before == after:
            continue
        if not name.endswith(".md"):
            raise ValueError(
                f"baseline executable source or upstream pin changed: {name}"
            )
        differences.append(
            {"path": name, "baseline_sha256": before, "current_sha256": after}
        )
    return {
        "record_sha256": hashes["baseline_record"],
        "binary_sha256": hashes["baseline"],
        "revision": record["revision"],
        "requested_target": target,
        "thinlto": thinlto,
        "documentation_differences": differences,
        "source_policy": "All recorded example and Rust sources must match except Markdown documentation. The current benchmark harness differs intentionally; its validators, shared measurement helpers, Python, Clang, Rust toolchain, and optional ThinLTO linker must match.",
    }


def workloads(
    codec, sizes: list[int], batch_digits: int, operations, baseline=None
) -> list[dict]:
    generator = random.Random(3917)
    captured = {}
    if baseline is not None:
        captured = {item["label"]: item for item in baseline["workloads"]}
        if len(captured) != len(baseline["workloads"]):
            raise ValueError("baseline workload labels must be distinct")
    result = []
    for size in sizes:
        generated = bytes(generator.randrange(48, 58) for _ in range(size))
        for operation in operations:
            label = f"{operation}-{size}"
            payload = generated
            if baseline is not None:
                if label not in captured:
                    raise ValueError(f"baseline does not contain workload {label}")
                original = captured[label]
                payload = base64.b64decode(original["input_base64"], validate=True)
                if (
                    original["operation"] != operation
                    or original["digits"] != size
                    or len(payload) != size
                    or original["input_sha256"] != validator.digest(payload)
                ):
                    raise ValueError(f"invalid baseline workload {label}")
                expected = validator.oracle(
                    codec, operation, payload, original["repetitions"]
                )
                if (
                    expected["stdout"] != original["expected_stdout"]
                    or expected["output"]
                    != base64.b64decode(
                        original["expected_output_base64"], validate=True
                    )
                    or validator.digest(expected["output"])
                    != original["expected_output_sha256"]
                ):
                    raise ValueError(f"baseline workload oracle changed: {label}")
            repetitions = max(1, batch_digits // size)
            expected = validator.oracle(codec, operation, payload, repetitions)
            result.append(
                {
                    "label": label,
                    "operation": operation,
                    "digits": size,
                    "repetitions": repetitions,
                    "input_base64": base64.b64encode(payload).decode(),
                    "input_sha256": validator.digest(payload),
                    "expected_stdout": expected["stdout"],
                    "expected_output_base64": base64.b64encode(
                        expected["output"]
                    ).decode(),
                    "expected_output_sha256": validator.digest(expected["output"]),
                }
            )
    return result


def measure(workload: dict, commands: dict, directory: Path, args) -> dict:
    payload = base64.b64decode(workload["input_base64"])
    expected = base64.b64decode(workload["expected_output_base64"])
    directories = {name: directory / name for name in commands}
    for path in directories.values():
        path.mkdir(parents=True)
        (path / "input.bin").write_bytes(payload)

    def run(name: str) -> dict:
        path = directories[name]
        source, output = path / "input.bin", path / "output.bin"
        output.write_bytes(b"unchanged output")
        command = [
            *commands[name],
            workload["operation"],
            str(source),
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
        assert process.returncode == 0 and not process.stderr, (name, process)
        assert process.stdout.decode() == workload["expected_stdout"], (
            name,
            process.stdout,
        )
        assert output.read_bytes() == expected, (name, "complete output mismatch")
        assert source.read_bytes() == payload, "input changed"
        return {
            "command": command,
            "elapsed_ns": elapsed,
            "returncode": 0,
            "stdout": process.stdout.decode(),
            "stderr": "",
            "output_sha256": validator.digest(expected),
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
        for left, right in (
            ("cpython", "relar"),
            ("relar", "rust"),
            ("cpython", "rust"),
            ("baseline", "relar"),
            ("baseline", "rust"),
            ("cpython", "baseline"),
        )
        if left in commands and right in commands
    }
    return {**workload, "gates": gates, **phases, "summary": summaries, **ratios}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--relar-build-record", type=Path)
    parser.add_argument("--python", default="python3")
    parser.add_argument("--rust", type=Path, required=True)
    parser.add_argument("--rust-build-record", type=Path)
    parser.add_argument("--clang", default="clang-18")
    parser.add_argument("--target")
    parser.add_argument("--lto-linker", type=Path)
    parser.add_argument("--artifact-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--samples", type=shared.positive_int)
    parser.add_argument("--warmups", type=shared.positive_int)
    parser.add_argument(
        "--batch-digits", type=shared.positive_int, default=16 * 1024 * 1024
    )
    parser.add_argument(
        "--sizes", nargs="+", type=shared.positive_int, default=[16, 256, 4096]
    )
    parser.add_argument(
        "--operations",
        nargs="+",
        choices=validator.OPERATIONS,
        default=validator.OPERATIONS,
    )
    parser.add_argument("--native-only", action="store_true")
    parser.add_argument("--baseline", type=Path, help="preserved Relar executable")
    parser.add_argument(
        "--baseline-record", type=Path, help="its original benchmark JSON"
    )
    parser.add_argument("--timeout", type=shared.positive_float, default=120)
    parser.add_argument("--cpu", type=int)
    parser.add_argument("--allow-dirty", action="store_true")
    args = parser.parse_args()
    if bool(args.baseline) != bool(args.baseline_record):
        parser.error("--baseline and --baseline-record must be supplied together")
    permutations = math.factorial(2 + (not args.native_only) + bool(args.baseline))
    if args.samples is None:
        args.samples = max(12, permutations)
    if args.warmups is None:
        args.warmups = max(6, permutations)
    if (
        args.samples < max(4, permutations)
        or args.samples % permutations
        or args.warmups % permutations
    ):
        parser.error(
            f"samples must be >= {max(4, permutations)}; samples and warmups must be multiples of {permutations}"
        )
    if len(set(args.sizes)) != len(args.sizes) or len(set(args.operations)) != len(
        args.operations
    ):
        parser.error("sizes and operations must not repeat")
    cpu = shared.pin_cpu(args.cpu)
    provenance = validator.verify_upstream()
    artifacts = args.artifact_dir.resolve()
    measurement_labels = [
        "empty-control",
        *(
            f"{operation}-{size}"
            for size in args.sizes
            for operation in args.operations
        ),
    ]
    if any(
        args.output.resolve().is_relative_to((artifacts / label).resolve())
        for label in measurement_labels
    ):
        parser.error("--output must not be inside a measurement input/output directory")
    compiler = Path(shared.executable(args.compiler))
    python = Path(shared.executable(args.python))
    rust = Path(shared.executable(str(args.rust)))
    clang = Path(shared.executable(args.clang))
    rust_record_path = (
        args.rust_build_record or rust.parent / "build-record.json"
    ).resolve()
    rust_record = json.loads(rust_record_path.read_text())
    paths = {
        "compiler": compiler,
        "runtime": args.runtime.resolve(strict=True),
        "python": python,
        "clang": clang,
        "rust": rust,
        "rust_build_record": rust_record_path,
        "harness": Path(__file__).resolve(),
        "validator": HERE / "luhn_validate.py",
        "shared_harness": HERE / "benchmark.py",
        "shared_validator": HERE / "validate.py",
    }
    baseline = None
    if args.baseline:
        paths["baseline"] = Path(shared.executable(str(args.baseline)))
        paths["baseline_record"] = args.baseline_record.resolve(strict=True)
        baseline = json.loads(paths["baseline_record"].read_text())
    paths.update(
        {
            "example:" + str(path.relative_to(validator.PACKAGE)): path
            for path in sorted(validator.PACKAGE.rglob("*"))
            if path.is_file() and "__pycache__" not in path.parts
        }
    )
    rust_sources = {
        str(path.relative_to(REFERENCE)): path
        for path in (
            REFERENCE / "Cargo.toml",
            REFERENCE / "Cargo.lock",
            REFERENCE / "build.py",
            *sorted((REFERENCE / "src").glob("*.rs")),
        )
    }
    paths.update({"rust_source:" + name: path for name, path in rust_sources.items()})
    assert rust_record["sources"] == {
        name: shared.sha256(path) for name, path in rust_sources.items()
    }
    assert all(
        validator.digest(rust_record["source_contents"][name].encode()) == value
        for name, value in rust_record["sources"].items()
    )
    assert rust_record["entrypoint"] == "luhn" and rust_record["input_guards_passed"]
    assert rust_record["binary_sha256"] == shared.sha256(rust)
    thinlto = args.lto_linker is not None
    assert rust_record["thinlto"] == thinlto, (
        "Rust and Relar must use matching build modes"
    )
    environment = rust_record["environment"]
    assert environment["CARGO_PROFILE_RELEASE_OPT_LEVEL"] == "3"
    assert environment["CARGO_PROFILE_RELEASE_LTO"] == ("thin" if thinlto else "off")
    assert rust_record["llvm_budget"] == (1000 if thinlto else None)
    expected_flags = (
        "-Cllvm-args=-import-instr-limit=1000 -Cllvm-args=-inline-threshold=1000"
        if thinlto
        else ""
    )
    assert environment["RUSTFLAGS"] == expected_flags
    assert environment["CARGO_ENCODED_RUSTFLAGS"] == (None if thinlto else "")
    if args.lto_linker:
        paths["lto_linker"] = args.lto_linker.absolute()
    relar_record = None
    if args.relar_build_record:
        paths["relar_build_record"] = args.relar_build_record.resolve(strict=True)
        relar_record = json.loads(paths["relar_build_record"].read_text())
        assert {shared.sha256(compiler), shared.sha256(paths["runtime"])} <= set(
            relar_record["sha256"].values()
        )
        for name, value in relar_record["source_sha256"].items():
            paths["relar_source:" + name] = ROOT / name
            assert shared.sha256(ROOT / name) == value
    binary, llvm, validation_path = (
        artifacts / "luhn",
        artifacts / "luhn.ll",
        artifacts / "validation.json",
    )
    baseline_validation_path = artifacts / "baseline-validation.json"
    outputs = [binary, llvm, validation_path, args.output.resolve()]
    if baseline is not None:
        outputs.append(baseline_validation_path)
    protected = list(paths.values())
    for output in outputs:
        assert all(
            output.resolve() != item.resolve()
            and not (output.exists() and item.exists() and output.samefile(item))
            for item in protected
        ), "output would replace a source, tool, or another output"
        protected.append(output)
    hashes = {name: shared.sha256(path) for name, path in paths.items()}
    baseline_info = (
        baseline_provenance(
            baseline, hashes, provenance, rust_record, args.target, thinlto
        )
        if baseline is not None
        else None
    )
    codec, _ = validator.load_upstream()
    selected_workloads = workloads(
        codec, args.sizes, args.batch_digits, args.operations, baseline
    )
    revision = run_checked(["git", "-C", str(ROOT), "rev-parse", "HEAD"]).strip()
    dirty = bool(run_checked(["git", "-C", str(ROOT), "status", "--porcelain"]).strip())
    if dirty and not args.allow_dirty:
        raise ValueError(
            "commit measured sources first, or use --allow-dirty for exploration"
        )
    python_info = json.loads(run_checked([str(python), "-I", "-c", shared.PYTHON_INFO]))
    assert python_info["implementation"] == "cpython"
    artifacts.mkdir(parents=True, exist_ok=True)
    command = [
        str(compiler),
        "build",
        str(validator.ENTRY),
        "--module-root",
        str(validator.PACKAGE),
        "-o",
        str(binary),
        "--runtime",
        str(paths["runtime"]),
        "--clang",
        str(clang),
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
    binary_hash, llvm_hash = shared.sha256(binary), shared.sha256(llvm)
    validation_command = [
        str(python),
        "-I",
        "-B",
        str(HERE / "luhn_validate.py"),
        "--binary",
        str(binary),
        "--rust",
        str(rust),
        "--output",
        str(validation_path),
    ]
    run_checked(validation_command)
    validation = json.loads(validation_path.read_text())
    assert validation["relar"]["binary_sha256"] == binary_hash
    assert validation["rust"]["binary_sha256"] == hashes["rust"]
    baseline_validation_command = None
    baseline_validation = None
    if baseline is not None:
        baseline_validation_command = [
            str(python),
            "-I",
            "-B",
            str(HERE / "luhn_validate.py"),
            "--binary",
            str(paths["baseline"]),
            "--output",
            str(baseline_validation_path),
        ]
        run_checked(baseline_validation_command)
        baseline_validation = json.loads(baseline_validation_path.read_text())
        assert baseline_validation["relar"]["binary_sha256"] == hashes["baseline"]
    commands = {"relar": [str(binary)], "rust": [str(rust)]}
    if baseline is not None:
        commands["baseline"] = [str(paths["baseline"])]
    if not args.native_only:
        commands["cpython"] = validator.python_command(str(python))
    record = {
        "schema": 1,
        "started_at_utc": datetime.now(timezone.utc).isoformat(),
        "revision": revision,
        "dirty": dirty,
        "upstream": provenance,
        "platform": platform.platform(),
        "cpu": cpu,
        "python": python_info,
        "compiler_version": run_checked([str(compiler), "--version"]).strip(),
        "clang_version": run_checked([str(clang), "--version"]).strip(),
        "source_and_tool_sha256": hashes,
        "binary_sha256": binary_hash,
        "llvm_sha256": llvm_hash,
        "compile_command": command,
        "build_ns": build_ns,
        "requested_target": args.target,
        "llvm_target": next(
            (
                line
                for line in llvm.read_text().splitlines()
                if line.startswith("target triple")
            ),
            None,
        ),
        "relar_build_record": relar_record,
        "rust_build_record": rust_record,
        "validation_command": validation_command,
        "validation": validation,
        "baseline_provenance": baseline_info,
        "baseline_record": baseline,
        "baseline_validation_command": baseline_validation_command,
        "baseline_validation": baseline_validation,
        "thinlto": thinlto,
        "batch_digits": args.batch_digits,
        "samples": args.samples,
        "warmups": args.warmups,
        "engine_orders": list(itertools.permutations(commands)),
        "ratio_interpretation": {
            "baseline_over_relar": "Preserved Relar median / current Relar median; higher means improvement.",
            "relar_over_rust": "Current Relar median / Rust median; lower is better.",
        },
        "workload_source": "preserved baseline inputs"
        if baseline
        else "seeded generator",
        "method": "Whole-process wall time, including startup and one input read/output write. Inputs are prepared and complete outputs verified outside timing. Identical repetitions and checksum contributions, with black_box per Rust call. All engine permutations are balanced. Empty-input controls report startup/protocol overhead without subtracting it.",
        "workloads": [],
    }
    empty = validator.oracle(codec, "checksum", b"")
    control = {
        "label": "empty-control",
        "operation": "checksum",
        "digits": 0,
        "repetitions": 1,
        "input_base64": "",
        "input_sha256": validator.digest(b""),
        "expected_stdout": empty["stdout"],
        "expected_output_base64": base64.b64encode(empty["output"]).decode(),
        "expected_output_sha256": validator.digest(empty["output"]),
    }
    record["startup_control"] = measure(
        control, commands, artifacts / "empty-control", args
    )
    args.output.parent.mkdir(parents=True, exist_ok=True)
    for workload in selected_workloads:
        result = measure(workload, commands, artifacts / workload["label"], args)
        result["startup_control_fraction"] = {
            name: record["startup_control"]["summary"][name]["median_seconds"]
            / summary["median_seconds"]
            for name, summary in result["summary"].items()
        }
        record["workloads"].append(result)
        args.output.write_text(json.dumps(record, indent=2) + "\n")
        print(
            result["label"]
            + ": "
            + ", ".join(
                f"{name}={value:.3f}x"
                for name, value in result.items()
                if "_over_" in name
            ),
            flush=True,
        )
    assert hashes == {name: shared.sha256(path) for name, path in paths.items()}
    assert (binary_hash, llvm_hash) == (shared.sha256(binary), shared.sha256(llvm))
    assert validator.verify_upstream() == provenance
    record["input_guards_passed"] = True
    record["completed_at_utc"] = datetime.now(timezone.utc).isoformat()
    args.output.write_text(json.dumps(record, indent=2) + "\n")


if __name__ == "__main__":
    main()
