"""Run a suite's own evidence entry at O0/O3 in ordinary and reachable mode."""
import argparse
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile


FORWARD = '''#!/bin/sh
case "$1" in
  build|check)
    opt=unspecified; previous=; has_reachable=false
    for arg do
      if [ "$previous" = --opt-level ]; then opt=$arg; fi
      case "$arg" in
        --opt-level=*) opt=${arg#*=} ;;
        -O[0-3]) opt=${arg#-O} ;;
        --reachable-only) has_reachable=true ;;
      esac
      previous=$arg
    done
    if [ "$IMRE_SUITE_MODE" = reachable ]; then
      if [ "$has_reachable" = false ]; then set -- "$@" --reachable-only; fi
    elif [ "$has_reachable" = true ]; then
      echo 'ordinary suite requested a reachable-only compilation' >&2; exit 2
    fi
    echo "$1-O$opt" >> "$IMRE_SUITE_LOG" || exit 1 ;;
esac
exec "$IMRE_SUITE_COMPILER" "$@"
'''


def sha256(path):
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


def mode_errors(report, reachable):
    """Check mode fields supplied by owners without inventing fields on older reports."""
    return [f"{row['id']} occurrence {row['occurrence']} O{part['opt_level']}"
            for part in json.loads(report.read_bytes()) for row in part["records"]
            if "reachable_only" in row.get("evidence", {})
            and row["evidence"]["reachable_only"] is not reachable]


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
    help_text = subprocess.check_output([sys.executable, entry, "--help"], cwd=root, text=True)
    has_option = re.search(r"(?m)^\s+--reachable-only(?:\s|,|$)", help_text) is not None
    with tempfile.TemporaryDirectory() as temp:
        # Older owners' runners already take --imre. Forward only Imre's compile
        # commands; the fixture, operations, assertions and result validator are
        # exactly the files at the requested source in both modes.
        forward = Path(temp) / "imre"
        forward.write_text(FORWARD)
        forward.chmod(0o700)
        for mode, report in zip(("ordinary", "reachable"), paths[:2], strict=True):
            log = Path(temp) / mode
            log.touch()
            command = [sys.executable, str(entry), "--imre", forward,
                       "--runtime", runtime, "--clang", args.clang.resolve(),
                       "--cpython-source", args.cpython_source.resolve(), "--result", report]
            if has_option and mode == "reachable":
                command.append("--reachable-only")
            status = subprocess.run(command, cwd=root, env={**os.environ,
                                    "IMRE_SUITE_COMPILER": str(compiler), "IMRE_SUITE_MODE": mode,
                                    "IMRE_SUITE_LOG": str(log)}).returncode
            commands = Counter(log.read_text().splitlines())
            result = {"runner_status": status, "validation_status": None,
                      "runner_mode_flag": has_option and mode == "reachable",
                      "compiler_commands": dict(commands), "mode_errors": []}
            for level in (0, 3):
                if not commands[f"build-O{level}"]:
                    result["mode_errors"].append(f"no native build observed at O{level}")
            if report.is_file():
                result["report_sha256"] = sha256(report)
                result["validation_status"] = subprocess.run(
                    [sys.executable, validator, reference, report], cwd=root).returncode
                if result["validation_status"] == 0:
                    result["mode_errors"].extend(mode_errors(report, mode == "reachable"))
            receipt["modes"][mode] = result
            paths[2].write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n")
    return int(any(r["runner_status"] != 0 or r["validation_status"] != 0 or r["mode_errors"]
                   for r in receipt["modes"].values()))


if __name__ == "__main__":
    sys.exit(main())
