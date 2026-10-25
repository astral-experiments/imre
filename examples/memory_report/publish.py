"""Publish complete text files by replacing a staged sibling entry."""

import tempfile
from pathlib import Path


def write_report(path: Path, rendered: str) -> None:
    with tempfile.TemporaryDirectory(dir=path.parent, prefix=".imre-report-") as name:
        staged = Path(name) / "report.md"
        staged.write_text(rendered, encoding="utf-8")
        staged.replace(path)


def write_capture(workspace: Path, directory: Path, role: str, stdout: str, stderr: str) -> None:
    staged_stdout = workspace / "stdout.txt"
    staged_stderr = workspace / "stderr.txt"
    staged_stdout.write_text(stdout, encoding="utf-8")
    staged_stderr.write_text(stderr, encoding="utf-8")
    staged_stdout.replace(directory / (role + ".stdout.txt"))
    staged_stderr.replace(directory / (role + ".stderr.txt"))
