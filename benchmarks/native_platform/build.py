"""Build the explicit native Codon OS/TOML boundary with retained provenance."""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import time


HERE = Path(__file__).resolve().parent


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--toolchain")
    parser.add_argument("--artifact-dir", required=True, type=Path)
    parser.add_argument("--target-dir", required=True, type=Path)
    parser.add_argument("--build-cache", required=True, type=Path)
    parser.add_argument("--thinlto", action="store_true")
    args = parser.parse_args()
    artifact = args.artifact_dir.resolve()
    target = args.target_dir.resolve()
    cache = args.build_cache.resolve()
    if target == cache:
        raise ValueError("target directory and shared build cache must differ")
    artifact.mkdir(parents=True, exist_ok=False)
    selected = ["+" + args.toolchain] if args.toolchain else []
    cargo = ["cargo", *selected, *(["-Zohm-defaults=no"] if args.toolchain == "ohm" else [])]
    environment = {**os.environ, "CARGO_TARGET_DIR": str(target), "CARGO_BUILD_BUILD_DIR": str(cache),
                   "CARGO_PROFILE_RELEASE_OPT_LEVEL": "3", "CARGO_PROFILE_RELEASE_CODEGEN_UNITS": "16",
                   "CARGO_PROFILE_RELEASE_LTO": "thin" if args.thinlto else "off",
                   "RUSTFLAGS": "", "CARGO_ENCODED_RUSTFLAGS": "",
                   "RUSTC_WRAPPER": "", "RUSTC_WORKSPACE_WRAPPER": ""}
    for name in ("RUSTC", "CARGO_BUILD_RUSTC", "CARGO_BUILD_RUSTC_WRAPPER", "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER", "CARGO_BUILD_TARGET"):
        environment.pop(name, None)
    if args.thinlto:
        environment.pop("CARGO_ENCODED_RUSTFLAGS")
        environment["RUSTFLAGS"] = "-Cllvm-args=-import-instr-limit=1000 -Cllvm-args=-inline-threshold=1000"

    def run(command):
        return subprocess.run(command, cwd=HERE, env=environment, capture_output=True,
                              text=True, check=True, timeout=1200)

    tools = {}
    for name in ("cargo", "rustc"):
        tools[name] = Path(run(["rustup", "which", name, *(["--toolchain", args.toolchain] if args.toolchain else [])]).stdout.strip()).resolve(strict=True)
    environment["RUSTC"] = str(tools["rustc"])
    rustc = run(["rustc", *selected, "--version", "--verbose"]).stdout
    host = next(line[6:] for line in rustc.splitlines() if line.startswith("host: "))
    environment["CARGO_BUILD_TARGET"] = host
    sources = {str(path.relative_to(HERE)): path for path in (HERE / "Cargo.toml", HERE / "Cargo.lock", HERE / "build.py", *sorted((HERE / "src").glob("*.rs")))}
    hashes = {name: digest(path) for name, path in sources.items()}
    tool_hashes = {name: digest(path) for name, path in tools.items()}
    run([*cargo, "fmt", "--check"])
    command = [*cargo, "build", "--release", "--locked", "--offline", "--verbose",
               "--message-format=json-render-diagnostics", "--target", host, "--target-dir", str(target)]
    for path in (HERE / "src").glob("*.rs"):
        path.touch()
    started = time.perf_counter_ns()
    result = run(command)
    elapsed = time.perf_counter_ns() - started
    messages = [json.loads(line) for line in result.stdout.splitlines() if line]
    outputs = [message for message in messages if message.get("reason") == "compiler-artifact"
               and message["target"]["name"] == "imre_benchmark_platform"]
    assert len(outputs) == 1 and outputs[0]["fresh"] is False
    libraries = [Path(path) for path in outputs[0]["filenames"] if path.endswith(".so")]
    assert len(libraries) == 1
    destination = artifact / libraries[0].name
    shutil.copy2(libraries[0], destination)
    assert hashes == {name: digest(path) for name, path in sources.items()}
    assert tool_hashes == {name: digest(path) for name, path in tools.items()}
    record = {"schema": 1, "kind": "native-benchmark-platform-build", "status": "passed",
              "completed_at_utc": datetime.now(timezone.utc).isoformat(),
              "sources": hashes, "source_contents": {name: path.read_text() for name, path in sources.items()},
              "tool_paths": {name: str(path) for name, path in tools.items()}, "tool_sha256": tool_hashes,
              "rustc_verbose": rustc, "target": host, "command": command,
              "environment": {name: value for name, value in environment.items() if name.startswith("CARGO_") or name in ("RUSTFLAGS", "RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER")},
              "thinlto": args.thinlto, "llvm_budget": 1000 if args.thinlto else None,
              "build_ns": elapsed, "build_stdout": result.stdout, "build_stderr": result.stderr,
              "library": {"path": str(destination), "sha256": digest(destination), "bytes": destination.stat().st_size},
              "scope": "native TOML, captured child processes/timeouts, temporary directories; no report computation, Python, or Imre runtime",
              "input_guards_passed": True}
    (artifact / "build-record.json").write_text(json.dumps(record, indent=2) + "\n")
    print(str(destination))


if __name__ == "__main__":
    main()
