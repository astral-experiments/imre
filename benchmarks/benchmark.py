"""Compare the unchanged route planner on CPython and Imre in fresh processes."""

import argparse
import hashlib
import json
import math
import os
import platform
import runpy
import shutil
import statistics
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
LOCAL = ROOT / "benchmarks" / "local"
# Load the adjacent module even when PYTHONSAFEPATH omits the script directory.
VALIDATOR = runpy.run_path(str(ROOT / "benchmarks" / "validate.py"))
SOURCE = VALIDATOR["SOURCE"]
input_text = VALIDATOR["input_text"]
validate = VALIDATOR["validate"]

# Do not include CONFIG_ARGS itself: it can contain private build paths. Report
# the relevant configure choices and whether the recorded flags contain them.
PYTHON_INFO = r"""
import json, platform, shlex, sys, sysconfig
config = shlex.split(sysconfig.get_config_var("CONFIG_ARGS") or "")
flags = " ".join(str(sysconfig.get_config_var(key) or "") for key in
                 ("PY_CFLAGS", "PY_CORE_CFLAGS", "PY_LDFLAGS", "LDFLAGS"))
jit = getattr(sys, "_jit", None)
def optional_call(object, name):
    method = getattr(object, name, None)
    return method() if callable(method) else None
print(json.dumps({
    "executable": sys.executable,
    "implementation": sys.implementation.name,
    "version": sys.version,
    "version_number": platform.python_version(),
    "gil_enabled": optional_call(sys, "_is_gil_enabled"),
    "free_threaded_build": bool(sysconfig.get_config_var("Py_GIL_DISABLED")),
    "jit_available": optional_call(jit, "is_available"),
    "jit_enabled": optional_call(jit, "is_enabled"),
    "debug_build": bool(sysconfig.get_config_var("Py_DEBUG")),
    "pgo": {
        "configure_enable_optimizations": "--enable-optimizations" in config,
        "profile_use_flag_present": "-fprofile-use" in flags or "-fprofile-instr-use" in flags,
    },
    "lto": {
        "configure_option": next((arg for arg in config if
            arg == "--without-lto" or arg == "--with-lto" or arg.startswith("--with-lto=")), None),
        "lto_flag_present": "-flto" in flags,
    },
}))
"""


def positive_int(value: str) -> int:
    result = int(value)
    if result <= 0:
        raise argparse.ArgumentTypeError("must be positive")
    return result


def positive_float(value: str) -> float:
    result = float(value)
    if not math.isfinite(result) or result <= 0:
        raise argparse.ArgumentTypeError("must be finite and positive")
    return result


def executable(value: str) -> str:
    found = shutil.which(value)
    if found is None:
        raise ValueError(f"executable not found: {value}")
    return str(Path(found).resolve())


def sha256(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def run(command: list[str], stdin: bytes, timeout: float) -> dict:
    start = time.perf_counter_ns()
    result = subprocess.run(command, input=stdin, capture_output=True, timeout=timeout)
    elapsed = time.perf_counter_ns() - start
    if result.returncode != 0 or result.stderr:
        raise RuntimeError(
            f"{Path(command[0]).name} failed ({result.returncode}): "
            f"{result.stderr.decode(errors='replace')}"
        )
    return {
        "elapsed_ns": elapsed,
        "stdout": result.stdout.decode("utf-8"),
        "stderr": "",
        "returncode": 0,
    }


def summary(samples: list[dict], implementation: str) -> dict:
    durations = [sample[implementation]["elapsed_ns"] / 1e9 for sample in samples]
    quartiles = (
        statistics.quantiles(durations, n=4, method="inclusive")
        if len(durations) > 1
        else durations * 3
    )
    return {
        "median_seconds": statistics.median(durations),
        "min_seconds": min(durations),
        "max_seconds": max(durations),
        "p25_seconds": quartiles[0],
        "p75_seconds": quartiles[2],
    }


def pin_cpu(requested: int | None) -> dict:
    status = Path("/proc/self/status").read_text()
    tracer = next(
        line.split(":", 1)[1].strip()
        for line in status.splitlines()
        if line.startswith("TracerPid:")
    )
    if int(tracer) != 0:
        raise RuntimeError("benchmark is being traced; rerun outside the ptrace sandbox")
    allowed = sorted(os.sched_getaffinity(0))
    cpu = allowed[0] if requested is None else requested
    if cpu not in allowed:
        raise ValueError(f"CPU {cpu} is outside the allowed affinity mask")
    os.sched_setaffinity(0, {cpu})
    model = next(
        (
            line.split(":", 1)[1].strip()
            for line in Path("/proc/cpuinfo").read_text().splitlines()
            if line.startswith("model name")
        ),
        platform.machine(),
    )
    return {
        "model": model,
        "allowed_cpus_before_pinning": allowed,
        "affinity": sorted(os.sched_getaffinity(0)),
    }


def benchmark(args: argparse.Namespace) -> dict:
    cpu = pin_cpu(args.cpu)
    compiler = executable(args.compiler)
    python = executable(args.python)
    clang = executable(args.clang)
    runtime = args.runtime.resolve(strict=True)
    artifacts = args.artifact_dir.resolve()
    artifacts.mkdir(parents=True, exist_ok=True)
    binary = artifacts / "route_planner"
    llvm = artifacts / "route_planner.ll"
    inputs = {
        "source": SOURCE,
        "compiler": Path(compiler),
        "runtime": runtime,
        "clang": Path(clang),
        "harness": Path(__file__).resolve(),
        "validator": ROOT / "benchmarks" / "validate.py",
    }
    if any(path in (binary, llvm) for path in inputs.values()):
        raise ValueError("artifact paths must not overwrite source or tool inputs")
    hashes = {name: sha256(path) for name, path in inputs.items()}
    python_info = json.loads(
        run([python, "-I", "-c", PYTHON_INFO], b"", args.timeout)["stdout"]
    )
    if python_info["implementation"] != "cpython":
        raise ValueError("--python must select CPython")
    python_path = Path(python_info.pop("executable")).resolve()
    inputs["python"] = python_path
    hashes["python"] = sha256(python_path)
    python_info["executable_name"] = python_path.name
    compiler_version = run([compiler, "--version"], b"", args.timeout)["stdout"].strip()
    clang_version = run([clang, "--version"], b"", args.timeout)["stdout"].strip()
    revision = run(
        ["git", "-C", str(ROOT), "rev-parse", "HEAD"], b"", args.timeout
    )["stdout"].strip()
    dirty = bool(
        run(["git", "-C", str(ROOT), "status", "--porcelain"], b"", args.timeout)["stdout"]
    )

    compile_command = [
        compiler,
        "build",
        str(SOURCE),
        "-o",
        str(binary),
        "--runtime",
        str(runtime),
        "--clang",
        clang,
        "--opt-level",
        "3",
        "--emit-llvm",
        str(llvm),
    ]
    compilation = run(compile_command, b"", args.timeout)
    inputs.update({"binary": binary, "llvm_ir": llvm})
    hashes.update({name: sha256(inputs[name]) for name in ("binary", "llvm_ir")})
    validation = validate(binary, python, SOURCE)
    commands = {"cpython": [python, "-I", str(SOURCE)], "imre": [str(binary)]}
    workloads = []
    for width in args.sizes:
        stdin = input_text(width, args.routes, args.seed).encode("ascii")
        expected = run(commands["cpython"], stdin, args.timeout)["stdout"]
        phases = {"warmups": [], "samples": []}
        for phase, count in (("warmups", args.warmups), ("samples", args.samples)):
            for index in range(count):
                order = ["cpython", "imre"] if index % 2 == 0 else ["imre", "cpython"]
                pair = {"index": index, "order": order}
                for implementation in order:
                    result = run(commands[implementation], stdin, args.timeout)
                    if result["stdout"] != expected:
                        raise AssertionError(
                            f"output mismatch for {implementation}, "
                            f"width {width}, {phase} {index}"
                        )
                    pair[implementation] = result
                phases[phase].append(pair)
        cpython = summary(phases["samples"], "cpython")
        imre = summary(phases["samples"], "imre")
        workloads.append(
            {
                "width": width,
                "routes": args.routes,
                "seed": args.seed,
                "input": stdin.decode("ascii"),
                "input_sha256": hashlib.sha256(stdin).hexdigest(),
                "expected_stdout": expected,
                **phases,
                "summary": {
                    "cpython": cpython,
                    "imre": imre,
                    "speedup_ratio_of_medians": (
                        cpython["median_seconds"] / imre["median_seconds"]
                    ),
                },
            }
        )
        print(
            f"{width}x{width}, {args.routes} routes: "
            f"CPython {cpython['median_seconds']:.6f}s, Imre {imre['median_seconds']:.6f}s, "
            f"{cpython['median_seconds'] / imre['median_seconds']:.2f}x",
            flush=True,
        )

    for name, path in inputs.items():
        if sha256(path) != hashes[name]:
            raise RuntimeError(
                f"{name} changed during the benchmark; discard these measurements"
            )
    return {
        "schema_version": 1,
        "completed_at_utc": datetime.now(timezone.utc).isoformat(),
        "git": {"revision": revision, "dirty": dirty},
        "sha256": hashes,
        "tools": {"imre": compiler_version, "clang": clang_version, "cpython": python_info},
        "host": {
            "os": platform.system(),
            "kernel_release": platform.release(),
            "architecture": platform.machine(),
            "cpu": cpu,
        },
        "options": {
            "sizes": args.sizes,
            "routes": args.routes,
            "seed": args.seed,
            "samples": args.samples,
            "warmups": args.warmups,
            "timeout_seconds": args.timeout,
            "llvm_optimization": "O3",
        },
        "method": {
            "timer": "time.perf_counter_ns around subprocess.run",
            "includes": "fresh process startup, stdin/stdout pipes, complete route-planner execution, and process exit",
            "excludes": "Rust compiler/runtime builds, AOT compilation, correctness validation, and warmups",
            "ordering": "paired samples alternate CPython first and Imre first, starting with CPython",
            "cpu": "harness and all child processes inherit the same single-CPU affinity",
            "cpython_command": [python_path.name, "-I", "examples/route_planner.py"],
            "imre_command": ["<artifact-dir>/route_planner"],
            "compile_command": [
                "imre", "build", "examples/route_planner.py",
                "-o", "<artifact-dir>/route_planner",
                "--runtime", "<runtime.a>",
                "--clang", Path(clang).name,
                "--opt-level", "3",
                "--emit-llvm", "<artifact-dir>/route_planner.ll",
            ],
        },
        "compilation": compilation,
        "artifacts": {"executable_bytes": binary.stat().st_size, "llvm_ir_bytes": llvm.stat().st_size},
        "clock": vars(time.get_clock_info("perf_counter")),
        "validation": validation,
        "workloads": workloads,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", required=True)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--output", type=Path, default=LOCAL / "route-planner.json")
    parser.add_argument("--artifact-dir", type=Path, default=LOCAL / "artifacts")
    parser.add_argument("--sizes", type=positive_int, nargs="+", default=[32, 128, 256])
    parser.add_argument("--routes", type=positive_int, default=3)
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--samples", type=positive_int, default=11)
    parser.add_argument("--warmups", type=positive_int, default=2)
    parser.add_argument("--cpu", type=int)
    parser.add_argument("--timeout", type=positive_float, default=120.0)
    args = parser.parse_args()
    result = benchmark(args)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    temporary = args.output.with_name(args.output.name + ".tmp")
    temporary.write_text(json.dumps(result, indent=2) + "\n")
    temporary.replace(args.output)
    print(f"Saved {args.output}")


if __name__ == "__main__":
    main()
