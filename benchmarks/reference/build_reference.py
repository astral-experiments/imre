"""Build Rust benchmark references and record their source and tool identities."""

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import time
from datetime import datetime, timezone
from pathlib import Path


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def reference_sources(root: Path) -> dict[str, Path]:
    """Include the shared builder in the self-contained source record."""
    paths = {
        str(path.relative_to(root)): path
        for path in (
            root / "Cargo.toml",
            root / "Cargo.lock",
            root / "build.py",
            *sorted((root / "src").glob("*.rs")),
        )
    }
    paths["build_reference.py"] = Path(__file__).resolve()
    return paths


def build_reference(
    root: Path,
    description: str,
    *,
    entrypoint: str,
    binaries: dict[str, str],
    comparison_domain: str,
) -> None:
    parser = argparse.ArgumentParser(description=description)
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
            cwd=root,
            env=environment,
            capture_output=True,
            text=True,
            check=True,
            timeout=1200,
        )

    paths = reference_sources(root)
    hashes = {name: digest(path) for name, path in paths.items()}
    contents = {name: path.read_text() for name, path in paths.items()}
    tools = {}
    for name in ("cargo", "rustc"):
        command = ["rustup", "which", name]
        if args.toolchain:
            command += ["--toolchain", args.toolchain]
        tools[name] = Path(run(command).stdout.strip()).resolve(strict=True)
    tool_hashes = {name: digest(path) for name, path in tools.items()}
    destinations = {role: artifacts / name for role, name in binaries.items()}
    output = artifacts / "build-record.json"
    protected = [*paths.values(), *tools.values()]
    for path in (*destinations.values(), output):
        if any(
            path.resolve() == item.resolve()
            or (path.exists() and item.exists() and path.samefile(item))
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
        "--verbose",
        "--message-format=json-render-diagnostics",
        "--target-dir",
        str(target),
    ]
    # Require a fresh Cargo artifact even when the build directory is shared.
    for path in sorted((root / "src").glob("*.rs")):
        path.touch()
    start = time.perf_counter_ns()
    built = run(command)
    build_ns = time.perf_counter_ns() - start
    messages = [json.loads(line) for line in built.stdout.splitlines() if line]
    cargo_artifacts = {}
    binary_records = {}
    for role, destination in destinations.items():
        matches = [
            message
            for message in messages
            if message.get("reason") == "compiler-artifact"
            and message["target"]["name"] == destination.name
            and message.get("executable")
        ]
        assert len(matches) == 1 and matches[0]["fresh"] is False
        cargo_artifacts[role] = matches[0]
        shutil.copy2(matches[0]["executable"], destination)
        binary_records[role] = {
            "name": destination.name,
            "sha256": digest(destination),
            "bytes": destination.stat().st_size,
        }
    assert hashes == {name: digest(path) for name, path in paths.items()}
    assert tool_hashes == {name: digest(path) for name, path in tools.items()}
    record = {
        "schema": 1,
        "entrypoint": entrypoint,
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
        "native_artifact": cargo_artifacts["reference"],
        "binary_sha256": binary_records["reference"]["sha256"],
        "binary_bytes": binary_records["reference"]["bytes"],
        "binaries": binary_records,
        "comparison_domain": comparison_domain,
        "input_guards_passed": True,
    }
    if "producer" in cargo_artifacts:
        record["producer_artifact"] = cargo_artifacts["producer"]
    output.write_text(json.dumps(record, indent=2) + "\n")
    print(f"Saved {output}")
