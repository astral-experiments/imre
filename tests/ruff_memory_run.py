"""Check the original memory reporter's subprocess arguments, streams, and files."""

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "examples/memory_report/upstream/memory_report.py"
CHILD = ROOT / "tests/fixtures/ruff_memory/child.py"
PROJECTS = ("flake8", "sphinx", "prefect", "trio")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--source", type=Path, default=SOURCE)
    parser.add_argument("--python", default=sys.executable)
    args = parser.parse_args()
    if sys.flags.optimize:
        raise RuntimeError("validation requires Python assertions")
    python = str(Path(shutil.which(args.python)).absolute())
    source = args.source.resolve()
    commands = [[python, str(source)], [str(args.binary.resolve())]]

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        tools = root / "tools"
        tools.mkdir()
        for name in ("uv", "ty-old", "ty-new"):
            path = tools / name
            path.write_bytes(f"#!{python}\n".encode() + CHILD.read_bytes())
            path.chmod(0o755)
        environment = {
            **os.environ,
            "PATH": str(tools) + os.pathsep + os.environ["PATH"],
            "IMRE_PYTHON": python,
            "MEMORY_SETUP_SCRIPT": str(source.with_name("setup_primer_project.py")),
            "PYTHONHASHSEED": "0",
            "NO_COLOR": "1",
        }
        for mode in ("new", "prepared", "failed"):
            results = []
            for command in commands:
                work = root / "work"
                work.mkdir()
                if mode != "new":
                    for name in PROJECTS:
                        (work / "projects with spaces" / name).mkdir(parents=True)
                flags = [
                    "run",
                    "--projects-dir",
                    "projects with spaces",
                    "--old-reports-dir",
                    "old",
                    "--new-reports-dir",
                    "new",
                    "--old-ty",
                    str(tools / "ty-old"),
                    "--new-ty",
                    str(tools / "ty-new"),
                ]
                report = work / "report.md"
                report.write_bytes(b"previous report\n")
                if mode != "prepared":
                    flags.extend(["--output", str(report)])
                run = subprocess.run(
                    [*command, *flags],
                    cwd=work,
                    capture_output=True,
                    check=False,
                    env={
                        **environment,
                        "MEMORY_FAIL": "1" if mode == "failed" else "0",
                    },
                )
                assert run.returncode == (1 if mode == "failed" else 0), run.stderr
                stderr = run.stderr
                if mode == "failed":
                    progress = stderr.split(b"Traceback (most recent call last):")[0]
                    progress = progress.removesuffix(stderr.splitlines(keepends=True)[-1])
                    last = stderr.splitlines()[-1].removeprefix(b"subprocess.")
                    assert (
                        last.startswith(b"CalledProcessError:")
                        and b"exit status 3" in last
                    ), stderr
                    stderr = progress, last
                files = {
                    str(path.relative_to(work)): path.read_bytes()
                    for path in work.glob("*/*.json")
                }
                if mode == "failed":
                    assert (
                        set(files) == {"old/flake8.json"}
                        and report.read_bytes() == b"previous report\n"
                    )
                else:
                    assert len(files) == 8
                    rendered = run.stdout if mode == "prepared" else report.read_bytes()
                    assert b"## Memory usage report" in rendered and all(
                        n.encode() in rendered for n in PROJECTS
                    )
                    for key, raw in files.items():
                        assert json.loads(raw)["total_bytes"] == (
                            99 if key.startswith("old/") else 20480
                        )
                results.append((run.stdout, stderr, files, report.read_bytes()))
                shutil.rmtree(work)
            assert results[0] == results[1], (mode, results)
        print(
            "Ruff's original run matched CPython for fresh setup, prepared projects, and a failing child"
        )


if __name__ == "__main__":
    main()
