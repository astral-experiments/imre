"""Build the untouched itertools test entry points in all four optimization/reachability modes."""

import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
from stage_tests import stage


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def build(args, root, source, level, reachable):
    binary = root.parent / "suite"
    command = [args.imre, "build", source, "--module-root", root, "--opt-level", str(level),
               "--no-cache", "--runtime", args.runtime, "--clang", args.clang, "-o", binary]
    if reachable:
        command.append("--reachable-only")
    compiled = subprocess.run(command, capture_output=True, text=True, timeout=180)

    def output(result):
        return {"exit": result.returncode, "stdout": result.stdout.replace(str(root), "Lib"),
                "stderr": result.stderr.replace(str(root), "Lib")}

    row = {"opt_level": level, "reachable_only": reachable, "build": output(compiled)}
    if compiled.returncode:
        row["tests_executed"] = 0
    else:
        row["run"] = output(subprocess.run([binary], capture_output=True, text=True, timeout=600))
    return row


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "cpython-git", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    pin = manifest["commit"]
    report = {"format_version": 1, "cpython_commit": pin, "compiler_sha256": digest(args.imre),
              "runtime_sha256": digest(args.runtime), "suites": {}}
    with tempfile.TemporaryDirectory(prefix="imre-itertools-whole-") as tmp:
        stage(Path(tmp))
        root = Path(tmp) / "Lib"
        for module in [manifest["module"], *manifest["companions"]]:
            rel = "Lib/" + module.replace(".", "/") + ".py"
            path = Path(tmp) / rel
            original = subprocess.check_output(["git", "-C", args.cpython_git, "show", f"{pin}:{rel}"])
            if path.read_bytes() != original:
                raise ValueError(f"{rel} differs from its pinned Git blob")
            modes = [build(args, root, path, level, reachable)
                     for level in (0, 3) for reachable in (False, True)]
            report["suites"][module] = {"path": rel, "git_blob": manifest["files"][rel],
                                        "source_sha256": digest(path), "modes": modes}
            for row in modes:
                print(f"{module} O{row['opt_level']} reachable={row['reachable_only']}: "
                      f"build exit {row['build']['exit']}")
    args.result.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
