"""Render frozen `git diff` and formatter errors with Ruff's ecosystem reporter."""

import sys
from pathlib import Path

from ruff_ecosystem.format import markdown_format_result
from ruff_ecosystem.projects import ClonedRepository, FormatOptions, Project, Repository
from ruff_ecosystem.types import Comparison, Diff, Result, ToolError


def replay(directory: Path, copies: int) -> str:
    completed: list[tuple[Project, Comparison]] = []
    errored: list[tuple[Project, BaseException]] = []
    rows = (directory / "projects.tsv").read_text(encoding="utf-8").splitlines()
    for copy in range(copies):
        for row in rows:
            owner, name, revision, preview, exclude, kind, filename = row.split("\t")
            project_name = name if copy == 0 else f"{name}-copy-{copy}"
            project = Project(
                Repository(owner, project_name, revision),
                format_options=FormatOptions(preview=preview == "1", exclude=exclude),
            )
            text = (directory / filename).read_bytes().decode("utf-8")
            if kind == "error":
                errored.append((project, ToolError(text)))
            else:
                repo = ClonedRepository(
                    owner, project_name, revision, revision, directory / name
                )
                completed.append((project, Comparison(Diff(text.splitlines()), repo)))
    return markdown_format_result(Result(errored, completed))


if __name__ == "__main__":
    print(replay(Path(sys.argv[1]), int(sys.argv[2])))
