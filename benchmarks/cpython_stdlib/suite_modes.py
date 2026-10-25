"""Run a suite's own evidence entry at O0/O3 in ordinary and reachable mode."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


def sha256(path):
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", required=True, help="exact Git commit of the suite checkout")
    for name in ("entry", "reference", "imre", "runtime", "clang", "cpython-source", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    root = Path(subprocess.check_output(["git", "rev-parse", "--show-toplevel"], text=True).strip())
    source = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    if source != args.source:
        parser.error(f"requested source {args.source}, checked out {source}")
    entry, reference = args.entry.resolve(), args.reference.resolve()
    validator = root / "benchmarks/cpython_stdlib/suite_evidence.py"
    subprocess.run(["git", "ls-files", "--error-unmatch", "--",
                    *(p.relative_to(root) for p in (entry, reference, validator))],
                   cwd=root, stdout=subprocess.DEVNULL, check=True)
    if subprocess.run(["git", "diff", "--quiet", "HEAD", "--"], cwd=root).returncode:
        parser.error("tracked checkout changes do not match the requested source")
    compiler, runtime = args.imre.resolve(), args.runtime.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    paths = [output / name for name in ("ordinary.json", "reachable.json", "source.json")]
    if any(path.exists() for path in paths):
        parser.error("output already contains a suite report; use a fresh directory")

    tree = subprocess.check_output(["git", "rev-parse", "HEAD^{tree}"], cwd=root, text=True).strip()
    receipt = {"suite_commit": source, "suite_tree": tree, "entry": str(entry.relative_to(root)),
               "entry_sha256": sha256(entry), "reference_sha256": sha256(reference),
               "validator_sha256": sha256(validator), "launcher_sha256": sha256(Path(__file__)),
               "compiler_sha256": sha256(compiler), "runtime_sha256": sha256(runtime),
               "python": sys.version.split()[0], "modes": {}}
    with tempfile.TemporaryDirectory() as temp:
        # Older owners' runners already take --imre. Forward only Imre's compile
        # commands; the fixture, operations, assertions and result validator are
        # exactly the files at the requested source in both modes.
        reachable = Path(temp) / "imre"
        reachable.write_text('#!/bin/sh\ncase "$1" in\n'
                             '  build|check) for arg do [ "$arg" != --reachable-only ] || '
                             'exec "$IMRE_SUITE_COMPILER" "$@"; done\n'
                             '    exec "$IMRE_SUITE_COMPILER" "$@" --reachable-only ;;\n'
                             '  *) exec "$IMRE_SUITE_COMPILER" "$@" ;;\nesac\n')
        reachable.chmod(0o700)
        for mode, executable, report in zip(("ordinary", "reachable"), (compiler, reachable), paths[:2]):
            command = [sys.executable, str(entry), "--imre", executable,
                       "--runtime", runtime, "--clang", args.clang.resolve(),
                       "--cpython-source", args.cpython_source.resolve(), "--result", report]
            status = subprocess.run(command, cwd=root, env={**os.environ,
                                    "IMRE_SUITE_COMPILER": str(compiler)}).returncode
            result = {"runner_status": status, "validation_status": None}
            if report.is_file():
                result["report_sha256"] = sha256(report)
                result["validation_status"] = subprocess.run(
                    [sys.executable, validator, reference, report], cwd=root).returncode
            receipt["modes"][mode] = result
            paths[2].write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n")
    return int(any(r["runner_status"] != 0 or r["validation_status"] != 0
                   for r in receipt["modes"].values()))


if __name__ == "__main__":
    sys.exit(main())
