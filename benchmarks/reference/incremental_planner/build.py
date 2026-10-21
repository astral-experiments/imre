"""Build Rust planner references and preserve their source/build provenance."""

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import time
from datetime import datetime, timezone
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
LOCAL = ROOT / "benchmarks/local/incremental-reference"


def digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def sources() -> dict:
    return {
        str(path.relative_to(HERE)): digest(path)
        for path in [
            HERE / "Cargo.toml",
            HERE / "Cargo.lock",
            *sorted((HERE / "src").rglob("*.rs")),
        ]
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--toolchain", help="rustup toolchain; use ohm on this devbox")
    entrypoints = parser.add_mutually_exclusive_group()
    entrypoints.add_argument(
        "--batch", action="store_true", help="build the BigInt batch entrypoint"
    )
    entrypoints.add_argument(
        "--manifest",
        action="store_true",
        help="build the BigInt file-manifest entrypoint",
    )
    parser.add_argument("--artifact-dir", type=Path)
    parser.add_argument(
        "--target-dir",
        type=Path,
        help="default: CARGO_TARGET_DIR or the mode-specific local target directory",
    )
    parser.add_argument(
        "--build-cache", type=Path, help="override CARGO_BUILD_BUILD_DIR"
    )
    parser.add_argument(
        "--llvm-budget",
        type=int,
        metavar="POSITIVE_INT",
        help="set LLVM import and inlining budgets explicitly",
    )
    parser.add_argument("--offline", action="store_true")
    args = parser.parse_args()
    if args.llvm_budget is not None and args.llvm_budget <= 0:
        parser.error("--llvm-budget must be a positive integer")
    entrypoint = (
        "manifest" if args.manifest else "batch" if args.batch else "incremental"
    )
    default_artifacts = ROOT / f"benchmarks/local/{entrypoint}-reference"
    artifacts = (args.artifact_dir or default_artifacts).resolve()
    target = (
        args.target_dir
        or Path(os.environ.get("CARGO_TARGET_DIR", default_artifacts / "target"))
    ).resolve()
    modes = (
        (("bigint", ["--features", entrypoint, "--bin", f"planner-{entrypoint}"]),)
        if args.batch or args.manifest
        else (
            ("bigint", []),
            ("i64", ["--no-default-features", "--features", "checked-i64"]),
        )
    )
    prefix = f"planner-{entrypoint}" if args.batch or args.manifest else "planner"
    environment = os.environ.copy()
    environment.update(
        CARGO_TARGET_DIR=str(target), RUSTFLAGS="", CARGO_ENCODED_RUSTFLAGS=""
    )
    if args.llvm_budget is not None:
        # Even an empty encoded value would take precedence over RUSTFLAGS.
        environment.pop("CARGO_ENCODED_RUSTFLAGS")
        environment["RUSTFLAGS"] = (
            f"-Cllvm-args=-import-instr-limit={args.llvm_budget} "
            f"-Cllvm-args=-inline-threshold={args.llvm_budget}"
        )
    if args.build_cache is not None:
        environment["CARGO_BUILD_BUILD_DIR"] = str(args.build_cache.resolve())
    cache = environment.get("CARGO_BUILD_BUILD_DIR")
    if cache is not None and Path(cache).resolve() == target:
        raise ValueError("target and shared build-cache directories must be distinct")
    cargo = ["cargo"]
    rustc = ["rustc"]
    if args.toolchain:
        cargo.append("+" + args.toolchain)
        rustc.append("+" + args.toolchain)
    if args.toolchain == "ohm":
        cargo.append("-Zohm-defaults=no")
    source_hashes = sources()
    source_contents = {name: (HERE / name).read_text() for name in source_hashes}
    script = Path(__file__).read_text()
    script_hash = digest(Path(__file__))
    outputs = [artifacts / f"{prefix}-{mode}" for mode, _ in modes]
    outputs.append(artifacts / "build-record.json")
    protected = {HERE / name for name in source_hashes} | {Path(__file__).resolve()}

    def public(text: str) -> str:
        for path, label in sorted(
            [
                (str(HERE), "<reference-root>"),
                (str(target), "<target-dir>"),
                (str(artifacts), "<artifact-dir>"),
                (str(ROOT), "<repository>"),
            ],
            key=lambda item: len(item[0]),
            reverse=True,
        ):
            text = text.replace(path, label)
        return text

    def run(command: list[str], quiet: bool = False) -> subprocess.CompletedProcess:
        result = subprocess.run(
            command,
            cwd=HERE,
            env=environment,
            capture_output=True,
            text=True,
            check=True,
            timeout=1200,
        )
        if result.stdout and not quiet:
            print(result.stdout, end="", flush=True)
        if result.stderr and not quiet:
            print(result.stderr, end="", flush=True)
        return result

    rustc_version = run(rustc + ["--version", "--verbose"]).stdout
    cargo_version = run(cargo + ["--version"]).stdout
    tool_paths = {}
    rustup = shutil.which("rustup")
    for name in ("rustc", "cargo"):
        path = Path(shutil.which(name)).resolve()
        if rustup is not None and path.samefile(rustup):
            command = [rustup, "which", name]
            if args.toolchain:
                command += ["--toolchain", args.toolchain]
            path = Path(run(command, quiet=True).stdout.strip()).resolve()
        tool_paths[name] = path
    tool_hashes = {name: digest(path) for name, path in tool_paths.items()}
    protected.update(tool_paths.values())
    for output in outputs:
        if any(
            output.resolve() == path.resolve()
            or (output.exists() and path.exists() and output.samefile(path))
            for path in protected
        ):
            raise ValueError(
                "build outputs must be distinct and must not overwrite inputs"
            )
        protected.add(output)
    artifacts.mkdir(parents=True, exist_ok=True)
    run(cargo + ["fmt", "--check"])
    binaries = {}
    for mode, features in modes:
        command = cargo + ["build", "--release", "--locked"]
        if args.offline:
            command.append("--offline")
        command += features
        started = time.perf_counter()
        result = run(command)
        elapsed = time.perf_counter() - started
        destination = artifacts / f"{prefix}-{mode}"
        executable = (
            f"planner-{entrypoint}"
            if args.batch or args.manifest
            else "planner-reference"
        )
        shutil.copy2(target / "release" / executable, destination)
        binaries[mode] = {
            "name": destination.name,
            "sha256": digest(destination),
            "bytes": destination.stat().st_size,
            "command": command,
            "build_seconds": elapsed,
            "build_stdout": public(result.stdout),
            "build_stderr": public(result.stderr),
        }
    if sources() != source_hashes or digest(Path(__file__)) != script_hash:
        raise RuntimeError(
            "Rust sources, lockfile, or build helper changed during the build"
        )
    for name, path in tool_paths.items():
        if digest(path) != tool_hashes[name]:
            raise RuntimeError(f"{name} changed during the build")
    record = {
        "completed_at_utc": datetime.now(timezone.utc).isoformat(),
        "entrypoint": entrypoint,
        "sources": source_hashes,
        "source_contents": source_contents,
        "binaries": binaries,
        "rustc_verbose": rustc_version,
        "cargo_version": cargo_version,
        "tool_sha256": tool_hashes,
        "tool_hash_scope": "rustup-selected binaries when using rustup shims; otherwise command entrypoints",
        "llvm_budget": args.llvm_budget,
        "environment": {
            "CARGO_TARGET_DIR": "<target-dir>",
            "CARGO_BUILD_BUILD_DIR": "<shared-build-cache>" if cache else None,
            "CARGO_HOME": "<cargo-home>" if environment.get("CARGO_HOME") else None,
            "RUSTFLAGS": environment["RUSTFLAGS"],
            "CARGO_ENCODED_RUSTFLAGS": environment.get("CARGO_ENCODED_RUSTFLAGS"),
            **{
                name: value
                for name, value in environment.items()
                if name.startswith("CARGO_PROFILE_RELEASE_")
                or name
                in (
                    "CARGO_UNSTABLE_OHM_PROC_MACRO_TRUST",
                    "CARGO_UNSTABLE_OHM_NATIVE_TOOL_TRUST",
                )
            },
        },
        "build_script_sha256": script_hash,
        "build_script": script,
        "profile": "Cargo.toml [profile.release] with any recorded CARGO_PROFILE_RELEASE_* overrides; inherited Rust flags cleared; optional explicit LLVM budgets; Ohm experimental defaults disabled when selected",
    }
    path = artifacts / "build-record.json"
    path.write_text(json.dumps(record, indent=2) + "\n")
    print(f"Saved {path}")


if __name__ == "__main__":
    main()
