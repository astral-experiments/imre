"""Build the independent Luhn reference and record its sources, tools, and flags."""

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


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def sources() -> dict[str, Path]:
    return {
        str(path.relative_to(HERE)): path
        for path in (
            HERE / "Cargo.toml",
            HERE / "Cargo.lock",
            *sorted((HERE / "src").glob("*.rs")),
        )
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--toolchain", help="pinned repository toolchain by default; ohm locally"
    )
    parser.add_argument("--artifact-dir", type=Path, required=True)
    parser.add_argument("--target-dir", type=Path)
    parser.add_argument("--build-cache", type=Path)
    parser.add_argument("--thinlto", action="store_true")
    args = parser.parse_args()
    artifacts = args.artifact_dir.resolve()
    target = (
        args.target_dir
        or Path(os.environ.get("CARGO_TARGET_DIR", artifacts / "target"))
    ).resolve()
    environment = {
        **os.environ,
        "CARGO_TARGET_DIR": str(target),
        "CARGO_PROFILE_RELEASE_OPT_LEVEL": "3",
        "CARGO_PROFILE_RELEASE_LTO": "thin" if args.thinlto else "off",
        "CARGO_PROFILE_RELEASE_CODEGEN_UNITS": "16",
        "RUSTFLAGS": "",
        "CARGO_ENCODED_RUSTFLAGS": "",
    }
    for name in ("RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"):
        environment.pop(name, None)
    if args.thinlto:
        environment.pop("CARGO_ENCODED_RUSTFLAGS")
        environment["RUSTFLAGS"] = (
            "-Cllvm-args=-import-instr-limit=1000 -Cllvm-args=-inline-threshold=1000"
        )
    if args.build_cache:
        environment["CARGO_BUILD_BUILD_DIR"] = str(args.build_cache.resolve())
    if (cache := environment.get("CARGO_BUILD_BUILD_DIR")) and Path(
        cache
    ).resolve() == target:
        raise ValueError("target directory and shared build cache must differ")
    selected = ["+" + args.toolchain] if args.toolchain else []
    cargo = ["cargo", *selected]
    if args.toolchain == "ohm":
        cargo.append("-Zohm-defaults=no")

    def run(command: list[str]) -> subprocess.CompletedProcess:
        return subprocess.run(
            command,
            cwd=HERE,
            env=environment,
            capture_output=True,
            text=True,
            check=True,
            timeout=1200,
        )

    paths = sources()
    paths["build.py"] = Path(__file__).resolve()
    hashes = {name: digest(path) for name, path in paths.items()}
    contents = {name: path.read_text() for name, path in paths.items()}
    tools = {}
    for name in ("cargo", "rustc"):
        command = ["rustup", "which", name]
        if args.toolchain:
            command += ["--toolchain", args.toolchain]
        tools[name] = Path(run(command).stdout.strip()).resolve(strict=True)
    tool_hashes = {name: digest(path) for name, path in tools.items()}
    destination, output = artifacts / "luhn-reference", artifacts / "build-record.json"
    protected = [*paths.values(), *tools.values()]
    for path in (destination, output):
        if any(
            path.resolve() == item.resolve() or (path.exists() and path.samefile(item))
            for item in protected
        ):
            raise ValueError("output would overwrite a source, tool, or other output")
        protected.append(path)
    artifacts.mkdir(parents=True, exist_ok=True)
    rustc = run(["rustc", *selected, "--version", "--verbose"]).stdout
    cargo_version = run([*cargo, "--version"]).stdout
    run([*cargo, "fmt", "--check"])
    command = [
        *cargo,
        "build",
        "--release",
        "--locked",
        "--offline",
        "--target-dir",
        str(target),
    ]
    start = time.perf_counter_ns()
    built = run(command)
    build_ns = time.perf_counter_ns() - start
    shutil.copy2(target / "release/luhn-reference", destination)
    assert hashes == {name: digest(path) for name, path in paths.items()}
    assert tool_hashes == {name: digest(path) for name, path in tools.items()}
    record = {
        "schema": 1,
        "entrypoint": "luhn",
        "completed_at_utc": datetime.now(timezone.utc).isoformat(),
        "sources": hashes,
        "source_contents": contents,
        "tool_sha256": tool_hashes,
        "rustc_verbose": rustc,
        "cargo_version": cargo_version,
        "command": command,
        "environment": {
            **{
                name: value
                for name, value in environment.items()
                if name.startswith("CARGO_PROFILE_RELEASE_")
            },
            **{
                name: environment.get(name)
                for name in (
                    "CARGO_TARGET_DIR",
                    "CARGO_BUILD_BUILD_DIR",
                    "RUSTFLAGS",
                    "CARGO_ENCODED_RUSTFLAGS",
                    "CARGO_UNSTABLE_OHM_PROC_MACRO_TRUST",
                    "CARGO_UNSTABLE_OHM_NATIVE_TOOL_TRUST",
                )
            },
        },
        "thinlto": args.thinlto,
        "llvm_budget": 1000 if args.thinlto else None,
        "build_ns": build_ns,
        "build_stdout": built.stdout,
        "build_stderr": built.stderr,
        "binary_sha256": digest(destination),
        "binary_bytes": destination.stat().st_size,
        "unicode_version": "16.0.0",
        "comparison_domain": "String inputs through the typed file protocol; Unicode16 decimal digits and printability. Rust uses independent streaming arithmetic and per-call black_box. All recorded outputs and diagnostics are compared exactly with CPython.",
        "input_guards_passed": True,
    }
    output.write_text(json.dumps(record, indent=2) + "\n")
    print(f"Saved {output}")


if __name__ == "__main__":
    main()
