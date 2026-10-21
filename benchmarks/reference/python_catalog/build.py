"""Build the independent catalog reference and retain source/tool identities."""

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

if sys.flags.optimize:
    raise SystemExit("build validation requires Python assertions; do not use -O")

HERE = Path(__file__).resolve().parent


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--toolchain")
    parser.add_argument("--artifact-dir", type=Path, required=True)
    parser.add_argument("--target-dir", type=Path, required=True)
    parser.add_argument("--build-cache", type=Path)
    parser.add_argument("--thinlto", action="store_true")
    args = parser.parse_args()
    destination = args.artifact_dir.resolve() / "python-catalog-reference"
    output = destination.with_name("build-record.json")
    target = args.target_dir.resolve()
    environment = {
        **os.environ,
        "CARGO_TARGET_DIR": str(target),
        "CARGO_PROFILE_RELEASE_OPT_LEVEL": "3",
        "CARGO_PROFILE_RELEASE_LTO": "thin" if args.thinlto else "off",
        "CARGO_PROFILE_RELEASE_CODEGEN_UNITS": "16",
        "RUSTFLAGS": "",
        "CARGO_ENCODED_RUSTFLAGS": "",
    }
    for name in ("RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "CARGO_BUILD_TARGET",
                 "CARGO_BUILD_RUSTC", "CARGO_BUILD_RUSTC_WRAPPER", "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER"):
        environment.pop(name, None)
    if args.thinlto:
        environment.pop("CARGO_ENCODED_RUSTFLAGS")
        environment["RUSTFLAGS"] = "-Cllvm-args=-import-instr-limit=1000 -Cllvm-args=-inline-threshold=1000"
    if args.build_cache:
        environment["CARGO_BUILD_BUILD_DIR"] = str(args.build_cache.resolve())
    if environment.get("CARGO_BUILD_BUILD_DIR") == str(target):
        raise ValueError("target and shared build directories must differ")
    selected = ["+" + args.toolchain] if args.toolchain else []
    cargo = ["cargo", *selected]
    if args.toolchain == "ohm":
        cargo.append("-Zohm-defaults=no")

    def run(command):
        return subprocess.run(command, cwd=HERE, env=environment, capture_output=True,
                              text=True, check=True, timeout=1200)

    paths = {str(path.relative_to(HERE)): path for path in
             (HERE / "Cargo.toml", HERE / "Cargo.lock", HERE / "build.py", *sorted((HERE / "src").glob("*.rs")))}
    hashes = {name: digest(path) for name, path in paths.items()}
    contents = {name: path.read_text() for name, path in paths.items()}
    tools = {}
    for name in ("cargo", "rustc"):
        command = ["rustup", "which", name]
        if args.toolchain:
            command += ["--toolchain", args.toolchain]
        tools[name] = Path(run(command).stdout.strip()).resolve(strict=True)
    rustc_verbose = run(["rustc", *selected, "--version", "--verbose"]).stdout
    host = next(line.removeprefix("host: ") for line in rustc_verbose.splitlines() if line.startswith("host: "))
    environment.update({"RUSTC": str(tools["rustc"]), "RUSTC_WRAPPER": "",
                        "RUSTC_WORKSPACE_WRAPPER": "", "CARGO_BUILD_TARGET": host})
    tool_hashes = {name: digest(path) for name, path in tools.items()}
    protected = [*paths.values(), *tools.values()]
    for path in (destination, output):
        if any(path.resolve() == other.resolve() or
               (path.exists() and other.exists() and path.samefile(other)) for other in protected):
            raise ValueError("output would overwrite a protected source or tool")
        protected.append(path)
    destination.parent.mkdir(parents=True, exist_ok=True)
    run([*cargo, "fmt", "--check"])
    command = [*cargo, "build", "--release", "--locked", "--offline", "--verbose",
               "--message-format=json-render-diagnostics", "--target", host, "--target-dir", str(target)]
    # The shared build cache outlives this checkout. Require a fresh local
    # package compilation and use Cargo's reported executable path.
    for path in sorted((HERE / "src").glob("*.rs")):
        path.touch()
    started = time.perf_counter_ns()
    built = run(command)
    elapsed = time.perf_counter_ns() - started
    artifacts = [value for line in built.stdout.splitlines() if line
                 and (value := json.loads(line)).get("reason") == "compiler-artifact"
                 and value["target"]["name"] == "python-catalog-reference"
                 and value.get("executable")]
    assert len(artifacts) == 1 and artifacts[0]["fresh"] is False
    shutil.copy2(artifacts[0]["executable"], destination)
    assert hashes == {name: digest(path) for name, path in paths.items()}
    assert tool_hashes == {name: digest(path) for name, path in tools.items()}
    record = {
        "schema": 1,
        "entrypoint": "python_catalog",
        "completed_at_utc": datetime.now(timezone.utc).isoformat(),
        "sources": hashes,
        "source_contents": contents,
        "tool_paths": {name: str(path) for name, path in tools.items()},
        "tool_sha256": tool_hashes,
        "rustc_verbose": rustc_verbose,
        "target": host,
        "cargo_version": run([*cargo, "--version"]).stdout,
        "command": command,
        "environment": {name: environment.get(name) for name in (
            "CARGO_TARGET_DIR", "CARGO_BUILD_BUILD_DIR", "CARGO_PROFILE_RELEASE_OPT_LEVEL",
            "CARGO_PROFILE_RELEASE_LTO", "CARGO_PROFILE_RELEASE_CODEGEN_UNITS", "RUSTFLAGS",
            "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_JOBS", "CARGO_UNSTABLE_OHM_PROC_MACRO_TRUST",
            "CARGO_UNSTABLE_OHM_NATIVE_TOOL_TRUST", "CARGO_BUILD_TARGET", "RUSTC",
            "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER")},
        "thinlto": args.thinlto,
        "llvm_budget": 1000 if args.thinlto else None,
        "build_ns": elapsed,
        "build_stdout": built.stdout,
        "build_stderr": built.stderr,
        "native_artifact": artifacts[0],
        "binary_sha256": digest(destination),
        "binary_bytes": destination.stat().st_size,
        "comparison_domain": (
            "The complete offline CPythonFinder plus render transformation over raw release NDJSON. "
            "Typed serde decoding, stable artifact URL sorting, static/musl filtering, first-on-equal "
            "priority selection with insertion order, native i64 with arbitrary-precision fallback, "
            "version/variant/triple sorting, and complete ASCII-escaped two-space JSON rendering. "
            "Known-field duplicates, malformed typed records, unpaired surrogates in known strings, "
            "and empty architecture variants are rejected explicitly. Other upstream network finders "
            "are outside this mode. No parsed records or selected builds are prepared outside the program."
        ),
        "input_guards_passed": True,
    }
    output.write_text(json.dumps(record, indent=2) + "\n")
    print(f"Saved {output}")


if __name__ == "__main__":
    main()
