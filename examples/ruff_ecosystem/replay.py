"""Replay captured `ruff check` output through Ruff's original ecosystem reporter."""

import sys
from pathlib import Path

from ruff_ecosystem.check import CHECK_SUMMARY_LINE_RE, markdown_check_result
from ruff_ecosystem.projects import CheckOptions, ClonedRepository, Project, Repository
from ruff_ecosystem.types import Comparison, Diff, Result


def diagnostics(path: Path) -> list[str]:
    lines = path.read_bytes().decode("utf-8").splitlines()
    return sorted(line for line in lines if not CHECK_SUMMARY_LINE_RE.match(line))


def replay(directory: Path, copies: int) -> str:
    completed: list[tuple[Project, Comparison]] = []
    rows = (directory / "projects.tsv").read_text(encoding="utf-8").splitlines()
    for copy in range(copies):
        for row in rows:
            owner, project_name, revision, old, new = row.split("\t")
            name = project_name if copy == 0 else f"{project_name}-copy-{copy}"
            project = Project(
                Repository(owner, name, revision),
                check_options=CheckOptions(select="ALL"),
            )
            repository = ClonedRepository(
                owner, name, revision, revision, directory / project_name
            )
            diff = Diff.from_pair(
                diagnostics(directory / old), diagnostics(directory / new)
            )
            completed.append((project, Comparison(diff, repository)))
    return markdown_check_result(Result([], completed))


if __name__ == "__main__":
    sys.stdout.write(replay(Path(sys.argv[1]), int(sys.argv[2])))
