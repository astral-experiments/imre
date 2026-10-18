"""Build the inventory BigInt reference with explicit, reproducible provenance."""

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


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def sources() -> dict[str, str]:
    return {
        str(path.relative_to(HERE)): digest(path)
        for path in (
            HERE / "Cargo.toml",
            HERE / "Cargo.lock",
            *sorted((HERE / "src").rglob("*.rs")),
        )
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--toolchain", help="normal pinned toolchain by default; ohm locally"
    )
    parser.add_argument(
        "--cargo-wrapper",
        type=Path,
        help="optional local POSIX-shell Cargo wrapper; recorded verbatim",
    )
    parser.add_argument(
        "--artifact-dir",
        type=Path,
        default=ROOT / "benchmarks/local/inventory-reference",
    )
    parser.add_argument("--target-dir", type=Path)
    parser.add_argument("--build-cache", type=Path)
    parser.add_argument("--llvm-budget", type=int)
    parser.add_argument("--offline", action="store_true")
    args = parser.parse_args()
    if args.llvm_budget is not None and args.llvm_budget <= 0:
        parser.error("--llvm-budget must be positive")
    artifacts = args.artifact_dir.resolve()
    target = (
        args.target_dir
        or Path(os.environ.get("CARGO_TARGET_DIR", artifacts / "target"))
    ).resolve()
    environment = {
        **os.environ,
        "CARGO_TARGET_DIR": str(target),
        "RUSTFLAGS": "",
        "CARGO_ENCODED_RUSTFLAGS": "",
    }
    if args.llvm_budget is not None:
        environment.pop("CARGO_ENCODED_RUSTFLAGS")
        environment["RUSTFLAGS"] = (
            f"-Cllvm-args=-import-instr-limit={args.llvm_budget} -Cllvm-args=-inline-threshold={args.llvm_budget}"
        )
    if args.build_cache:
        environment["CARGO_BUILD_BUILD_DIR"] = str(args.build_cache.resolve())
    cache = environment.get("CARGO_BUILD_BUILD_DIR")
    if cache and Path(cache).resolve() == target:
        raise ValueError("target and shared build cache must be distinct")
    cargo = ["cargo"] + (["+" + args.toolchain] if args.toolchain else [])
    rustc = ["rustc"] + (["+" + args.toolchain] if args.toolchain else [])
    if args.toolchain == "ohm":
        cargo.append("-Zohm-defaults=no")
    wrapper = args.cargo_wrapper.resolve(strict=True) if args.cargo_wrapper else None
    if wrapper:
        cargo = ["sh", str(wrapper)]
    hashes = sources()
    contents = {name: (HERE / name).read_text() for name in hashes}
    script = Path(__file__).read_text()
    script_hash = digest(Path(__file__))
    wrapper_info = (
        {"sha256": digest(wrapper), "source": wrapper.read_text()} if wrapper else None
    )

    def run(command: list[str]) -> subprocess.CompletedProcess:
        result = subprocess.run(
            command,
            cwd=HERE,
            env=environment,
            capture_output=True,
            text=True,
            check=True,
            timeout=1200,
        )
        return result

    rustc_version = run(rustc + ["--version", "--verbose"]).stdout
    cargo_version = run(cargo + ["--version"]).stdout
    tool_paths = {}
    rustup = shutil.which("rustup")
    for name in ("cargo", "rustc"):
        path = Path(shutil.which(name)).resolve()
        if rustup and path.samefile(rustup):
            command = [rustup, "which", name] + (
                ["--toolchain", args.toolchain] if args.toolchain else []
            )
            path = Path(run(command).stdout.strip()).resolve()
        tool_paths[name] = path
    tool_hashes = {name: digest(path) for name, path in tool_paths.items()}
    protected = [
        *(HERE / name for name in hashes),
        Path(__file__).resolve(),
        *tool_paths.values(),
    ]
    if wrapper:
        protected.append(wrapper)
    destination = artifacts / "inventory-bigint"
    record_path = artifacts / "build-record.json"
    for output in (destination, record_path):
        if any(
            output.resolve() == path.resolve()
            or (output.exists() and path.exists() and output.samefile(path))
            for path in protected
        ):
            raise ValueError(
                "build outputs must not overwrite sources, tools, or each other"
            )
        protected.append(output)
    artifacts.mkdir(parents=True, exist_ok=True)
    run(cargo + ["fmt", "--check"])
    # Explicit --target-dir also overrides wrappers that set CARGO_TARGET_DIR.
    command = cargo + ["build", "--release", "--locked", "--target-dir", str(target)]
    if args.offline:
        command.append("--offline")
    start = time.perf_counter()
    result = run(command)
    elapsed = time.perf_counter() - start
    shutil.copy2(target / "release/inventory-reference", destination)
    if sources() != hashes or digest(Path(__file__)) != script_hash:
        raise RuntimeError("reference source/build helper changed during build")
    if any(digest(path) != tool_hashes[name] for name, path in tool_paths.items()):
        raise RuntimeError("Rust tool changed during build")
    if wrapper and digest(wrapper) != wrapper_info["sha256"]:
        raise RuntimeError("Cargo wrapper changed during build")

    def public(value: str) -> str:
        for path, label in sorted(
            (
                (str(HERE), "<reference-root>"),
                (str(target), "<target-dir>"),
                (str(artifacts), "<artifact-dir>"),
                (str(ROOT), "<repository>"),
            ),
            key=lambda pair: len(pair[0]),
            reverse=True,
        ):
            value = value.replace(path, label)
        return value

    record = {
        "completed_at_utc": datetime.now(timezone.utc).isoformat(),
        "entrypoint": "inventory",
        "sources": hashes,
        "source_contents": contents,
        "binaries": {
            "bigint": {
                "name": destination.name,
                "sha256": digest(destination),
                "bytes": destination.stat().st_size,
                "command": [public(part) for part in command],
                "build_seconds": elapsed,
                "build_stdout": public(result.stdout),
                "build_stderr": public(result.stderr),
            }
        },
        "rustc_verbose": rustc_version,
        "cargo_version": cargo_version,
        "tool_sha256": tool_hashes,
        "tool_hash_scope": "rustup-selected actual Rust tools; any Cargo wrapper is separately recorded",
        "cargo_wrapper": wrapper_info,
        "llvm_budget": args.llvm_budget,
        "environment": {
            "CARGO_TARGET_DIR": "<target-dir; explicit build CLI wins over wrapper environment>",
            "CARGO_BUILD_BUILD_DIR": "<shared-build-cache>" if cache else None,
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
        "environment_scope": "Helper invocation environment before the separately recorded wrapper; explicit --target-dir controls the artifact path even if a wrapper sets CARGO_TARGET_DIR.",
        "build_script_sha256": script_hash,
        "build_script": script,
        "profile": "Cargo.toml release profile plus recorded CARGO_PROFILE_RELEASE_* overrides; inherited Rust flags cleared; explicit optional LLVM budgets; selected wrapper retained verbatim",
    }
    record_path.write_text(json.dumps(record, indent=2) + "\n")
    print(result.stderr, end="")
    print(f"Saved {record_path}")


if __name__ == "__main__":
    main()
