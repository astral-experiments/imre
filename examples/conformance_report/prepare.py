"""Capture two ty runs for the offline conformance report (outside timing)."""

import argparse
import hashlib
import json
import subprocess
import tomllib
from pathlib import Path


TYPING_REVISION = "835b70f3f795f8e061b1f7621178515b0a77a0ad"


def digest(filename: Path) -> str:
    return hashlib.sha256(filename.read_bytes()).hexdigest()


def prepare(suite: Path, old_ty: Path, new_ty: Path, destination: Path, revision: str) -> dict:
    suite = suite.resolve(strict=True)
    old_ty = old_ty.resolve(strict=True)
    new_ty = new_ty.resolve(strict=True)
    destination = destination.resolve()
    if destination.exists():
        raise ValueError("preparation output directory must not already exist")
    groups_file = suite / "src" / "test_groups.toml"
    groups = tomllib.loads(groups_file.read_text(encoding="utf-8"))
    tests = suite / "tests"
    # Preserve upstream discovery order: .py files first, then .pyi files.
    files = [path.resolve() for path in [*tests.glob("*.py"), *tests.glob("*.pyi")]
             if path.name.split("_")[0] in groups]
    if not files:
        raise ValueError("no conformance test files discovered")
    destination.mkdir(parents=True)
    home = destination / "home"
    home.mkdir()
    environment = {
        "PATH": "/usr/bin:/bin", "HOME": str(home), "LANG": "C.UTF-8",
        "LC_ALL": "C.UTF-8", "TZ": "UTC", "PYTHONHASHSEED": "0", "PYTHON_JIT": "0",
    }
    # These are the pinned upstream command's arguments. Child execution and
    # input discovery are preparation; all report computation remains in the CLI.
    common = [
        "check", "--python-version=3.12", "--output-format=gitlab",
        "--ignore=assert-type-unspellable-subtype",
        "--error=ambiguous-protocol-member", "--error=invalid-enum-member-annotation",
        "--error=invalid-legacy-positional-parameter", "--error=mismatched-type-name",
        "--error=invalid-named-tuple-override", "--error=deprecated",
        "--error=redundant-final-classvar", "--exit-zero",
        f"--extra-search-path={tests}", *map(str, files),
    ]
    source_hashes = {str(path.relative_to(suite)): digest(path)
                     for path in sorted(suite.rglob("*")) if path.is_file()}
    tools = {"old": {"path": str(old_ty), "sha256": digest(old_ty)},
             "new": {"path": str(new_ty), "sha256": digest(new_ty)}}
    captures = {}
    aliases = {}
    for name, binary in (("old", old_ty), ("new", new_ty)):
        command = [str(binary), *common]
        completed = subprocess.run(command, cwd=suite, env=environment,
                                   capture_output=True, timeout=15)
        (destination / f"{name}.json").write_bytes(completed.stdout)
        (destination / f"{name}.stderr").write_bytes(completed.stderr)
        version = subprocess.run([str(binary), "--version"], cwd=suite, env=environment,
                                 capture_output=True, check=True, text=True).stdout.strip()
        captures[name] = {
            "command": command, "cwd": str(suite), "returncode": completed.returncode,
            "stdout_sha256": hashlib.sha256(completed.stdout).hexdigest(),
            "stderr_sha256": hashlib.sha256(completed.stderr).hexdigest(),
            "stdout_bytes": len(completed.stdout), "stderr_bytes": len(completed.stderr),
            "version": version,
        }
        completed.check_returncode()
        diagnostics = json.loads(completed.stdout)
        for diagnostic in diagnostics:
            raw = diagnostic["location"]["path"]
            original = Path(raw)
            aliases[raw] = str((original if original.is_absolute() else suite / original).resolve())
        captures[name]["diagnostics"] = len(diagnostics)
        captures[name]["major_diagnostics"] = sum(item["severity"] == "major" for item in diagnostics)
    manifest = {
        "suite_revision": revision,
        "test_files": list(map(str, files)),
        "diagnostic_paths": [{"raw": raw, "path": canonical} for raw, canonical in sorted(aliases.items())],
    }
    (destination / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    assert source_hashes == {str(path.relative_to(suite)): digest(path)
                             for path in sorted(suite.rglob("*")) if path.is_file()}
    assert tools["old"]["sha256"] == digest(old_ty) and tools["new"]["sha256"] == digest(new_ty)
    record = {
        "schema": 1, "suite_repository": "https://github.com/python/typing",
        "suite_revision": revision, "suite_directory": str(suite),
        "source_sha256": source_hashes, "tools": tools, "captures": captures,
        "environment": environment, "test_files": len(files),
        "test_source_bytes": sum(path.stat().st_size for path in files),
        "manifest_sha256": digest(destination / "manifest.json"),
        "preparation_source_sha256": digest(Path(__file__)), "guards_passed": True,
    }
    (destination / "preparation.json").write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")
    return record


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--suite", required=True, type=Path, help="python/typing's conformance directory")
    parser.add_argument("--suite-revision", default=TYPING_REVISION, help="revision used in report links and provenance")
    parser.add_argument("--old-ty", required=True, type=Path)
    parser.add_argument("--new-ty", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    args = parser.parse_args()
    record = prepare(args.suite, args.old_ty, args.new_ty, args.output_dir, args.suite_revision)
    print(json.dumps({"test_files": record["test_files"], "captures": record["captures"]}, indent=2))


if __name__ == "__main__":
    main()
