"""Build and freeze the compiler/runtime with exact source and toolchain provenance."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def sources():
    paths = [ROOT / name for name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml")]
    paths += [*ROOT.glob("crates/**/*.rs"), *ROOT.glob("crates/*/Cargo.toml")]
    paths += [path for path in (ROOT / ".cargo/config", ROOT / ".cargo/config.toml") if path.is_file()]
    return {str(path.relative_to(ROOT)): digest(path) for path in sorted(paths)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--toolchain", help="repository pin by default; use ohm on an Ohm devbox")
    parser.add_argument("--target-dir", type=Path, required=True)
    parser.add_argument("--shared-build-dir", type=Path, required=True)
    parser.add_argument("--rustup-home", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--jobs", type=int, default=4)
    parser.add_argument("--modes", nargs="+", choices=("normal", "thinlto", "asan"), default=["normal", "thinlto", "asan"])
    parser.add_argument("--allow-dirty", action="store_true")
    args = parser.parse_args()
    target, shared, output = (path.resolve() for path in (args.target_dir, args.shared_build_dir, args.output))
    if args.jobs <= 0 or len(set(args.modes)) != len(args.modes):
        parser.error("select a positive job count and distinct build modes")
    if target.is_relative_to(shared) or shared.is_relative_to(target):
        parser.error("the worktree target and shared build directories must be separate")
    if output.exists() or any(output.is_relative_to(path) or path.is_relative_to(output)
                              for path in (target, shared)):
        parser.error("output must be a new directory separate from Cargo's target and shared cache")
    hashes = sources()
    if any((ROOT / name).is_relative_to(output) for name in hashes):
        parser.error("output must not contain source inputs")
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    matches = True
    for name, expected in hashes.items():
        committed = subprocess.run(["git", "show", revision + ":" + name], cwd=ROOT, capture_output=True)
        matches &= committed.returncode == 0 and hashlib.sha256(committed.stdout).hexdigest() == expected
    if not matches and not args.allow_dirty:
        parser.error("compiler/runtime sources differ from HEAD; use --allow-dirty only for exploratory builds")
    settings = {
        "CARGO_TARGET_DIR": str(target), "CARGO_BUILD_BUILD_DIR": str(shared),
        "CARGO_BUILD_JOBS": str(args.jobs), "CARGO_PROFILE_RELEASE_OPT_LEVEL": "3",
        "CARGO_PROFILE_RELEASE_CODEGEN_UNITS": "16", "CARGO_PROFILE_RELEASE_LTO": "off",
        "RUSTFLAGS": "", "RUSTC_WRAPPER": "", "RUSTC_WORKSPACE_WRAPPER": "",
    }
    if args.toolchain == "ohm":
        settings.update(CARGO_UNSTABLE_OHM_PROC_MACRO_TRUST="all", CARGO_UNSTABLE_OHM_NATIVE_TOOL_TRUST="all")
    if args.rustup_home:
        settings["RUSTUP_HOME"] = str(args.rustup_home.resolve(strict=True))
    environment = {key: value for key, value in os.environ.items()
                   if key not in {"RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUSTC_BOOTSTRAP", "RUSTUP_TOOLCHAIN",
                                  "RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER",
                                  "CARGO_BUILD_TARGET", "CARGO_BUILD_RUSTC",
                                  "CARGO_BUILD_RUSTC_WRAPPER", "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER"}
                   and not key.startswith(("CARGO_PROFILE_", "CARGO_TARGET_"))}
    environment.update(settings)
    selected = ["+" + args.toolchain] if args.toolchain else []
    cargo_command = ["cargo", *selected, *(["-Zohm-defaults=no"] if args.toolchain == "ohm" else [])]
    tool_paths = {name: Path(subprocess.check_output(
        ["rustup", "which", name, *(["--toolchain", args.toolchain] if args.toolchain else [])],
        cwd=ROOT, env=environment, text=True
    ).strip()).resolve(strict=True) for name in ("rustc", "cargo")}
    tool_hashes = {name: digest(path) for name, path in tool_paths.items()}
    settings["RUSTC"] = str(tool_paths["rustc"])
    environment.update(settings)
    rustc = subprocess.check_output(["rustc", *selected, "-Vv"], cwd=ROOT, env=environment, text=True).strip()
    cargo = subprocess.check_output([*cargo_command, "-Vv"], cwd=ROOT, env=environment, text=True).strip()
    host = next(line[6:] for line in rustc.splitlines() if line.startswith("host: "))
    common = ["build", "--target", host, "--locked", "--message-format=json-render-diagnostics"]
    steps = [("normal", [*common, "--release", "--workspace"], {})]
    if "thinlto" in args.modes:
        steps.append(("thinlto", [*common, "-p", "imre-runtime", "--profile", "imre-tools-lto",
                                 "--config", 'profile.imre-tools-lto.inherits="release"'],
                      {"RUSTFLAGS": "-Clinker-plugin-lto"}))
    if "asan" in args.modes:
        steps.append(("asan", [*common, "-p", "imre-runtime", "--profile", "imre-tools-asan",
                              "--config", 'profile.imre-tools-asan.inherits="release"'],
                      {"RUSTC_BOOTSTRAP": "1", "RUSTFLAGS": "-Zsanitizer=address -Cforce-frame-pointers=yes"}))
    output.mkdir(parents=True, exist_ok=False)
    shutil.copy2(__file__, output / "build.py")
    for name in hashes:
        destination = output / "sources" / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / name, destination)
    record = {"schema": 1, "kind": "native-tools-build", "status": "running", "revision": revision,
              "source_matches_revision": matches, "input_guards_passed": False,
              "source_sha256": hashes, "sha256": {}, "environment": settings, "steps": [],
              "toolchain": rustc, "cargo": cargo, "target": host,
              "requested_toolchain": args.toolchain,
              "tool_paths": {name: str(path) for name, path in tool_paths.items()},
              "tool_sha256": tool_hashes, "driver_sha256": digest(output / "build.py")}
    def save():
        (output / "build-record.json").write_text(json.dumps(record, indent=2) + "\n")
    save()
    try:
        for mode, arguments, overrides in steps:
            command = [*cargo_command, *arguments]
            log = output / (mode + ".log")
            messages = output / (mode + ".jsonl")
            start = time.monotonic()
            with log.open("w") as stream, messages.open("w") as cargo_output:
                result = subprocess.run(command, cwd=ROOT, env={**environment, **overrides},
                                        stdout=cargo_output, stderr=stream, check=False)
            record["steps"].append({"name": mode, "command": command, "overrides": overrides,
                                    "log": str(log), "log_sha256": digest(log), "returncode": result.returncode,
                                    "messages": str(messages), "messages_sha256": digest(messages),
                                    "seconds": time.monotonic() - start})
            save()
            if result.returncode:
                raise RuntimeError(f"{mode} failed; see {log}")
            artifacts = {}
            for line in messages.read_text().splitlines():
                message = json.loads(line)
                if message.get("reason") != "compiler-artifact":
                    continue
                manifest = Path(message["manifest_path"]).resolve()
                if mode == "normal" and manifest == ROOT / "crates/imre-compiler/Cargo.toml" and message["target"]["name"] == "imre" and message.get("executable"):
                    artifacts["compiler"] = Path(message["executable"])
                if manifest == ROOT / "crates/imre-runtime/Cargo.toml":
                    for filename in message["filenames"]:
                        if Path(filename).name == "libimre_runtime.a":
                            artifacts[mode + "_runtime"] = Path(filename)
            required = {mode + "_runtime"} | ({"compiler"} if mode == "normal" else set())
            if set(artifacts) != required:
                raise RuntimeError(f"Cargo did not report the expected artifacts for {mode}: {artifacts}")
            record["steps"][-1]["artifacts"] = {name: str(path) for name, path in artifacts.items()}
            for name, source in artifacts.items():
                destination = output / ("imre" if name == "compiler" else name + ".a")
                before = digest(source)
                shutil.copy2(source, destination)
                if digest(source) != before or digest(destination) != before:
                    raise RuntimeError(f"build output changed while copying: {source}")
                record["sha256"][name] = before
            print(json.dumps({"mode": mode, "seconds": record["steps"][-1]["seconds"]}), flush=True)
        if hashes != sources():
            raise RuntimeError("compiler/runtime sources changed during the build")
        if record["driver_sha256"] != digest(Path(__file__)) or record["driver_sha256"] != digest(output / "build.py"):
            raise RuntimeError("build driver changed during the build")
        if tool_hashes != {name: digest(path) for name, path in tool_paths.items()}:
            raise RuntimeError("Rust toolchain changed during the build")
        if any(digest(output / "sources" / name) != expected for name, expected in hashes.items()):
            raise RuntimeError("retained sources differ from the build inputs")
        if revision != subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip():
            raise RuntimeError("Git revision changed during the build")
        record.update(status="passed", input_guards_passed=True)
    except BaseException as error:
        record.update(status="failed", error=str(error))
        raise
    finally:
        save()


if __name__ == "__main__":
    main()
