"""Collect reports from already prepared project trees with native subprocesses."""

import os
import subprocess
from dataclasses import dataclass
from pathlib import Path

from relar.json import decode

from .model import MemoryReport, ProjectComparison


@dataclass(slots=True)
class PreparedProject:
    name: str
    cwd: str
    argv: list[str]


@dataclass(slots=True)
class PreparedProjects:
    projects: list[PreparedProject]


def collect_report(project: PreparedProject, executable: str, environment: dict[str, str]) -> MemoryReport:
    command = [executable if argument == "{ty}" else argument for argument in project.argv]
    result = subprocess.run(
        command, cwd=project.cwd, env=environment, capture_output=True,
        text=True, encoding="utf-8", check=True,
    )
    return decode(result.stdout, MemoryReport)


def run_prepared(path: Path, old_ty: str, new_ty: str) -> list[ProjectComparison]:
    if old_ty[:1] != "/" or new_ty[:1] != "/":
        raise ValueError("--old-ty and --new-ty must be absolute executable paths")
    manifest = decode(path.read_text(encoding="utf-8"), PreparedProjects)
    if not manifest.projects:
        raise ValueError("prepared-project manifest must contain at least one project")
    names: set[str] = set()
    for project in manifest.projects:
        if not project.name or project.name in names:
            raise ValueError("prepared project names must be nonempty and unique")
        names.add(project.name)
        if not project.cwd:
            raise ValueError("prepared project cwd must be nonempty")
        if not project.argv or not any(argument == "{ty}" for argument in project.argv):
            raise ValueError("prepared project argv must contain a {ty} placeholder")
    environment = os.environ.copy()
    environment["TY_MEMORY_REPORT"] = "json"
    environment["TY_MAX_PARALLELISM"] = "1"
    comparisons: list[ProjectComparison] = []
    for project in manifest.projects:
        old = collect_report(project, old_ty, environment)
        new = collect_report(project, new_ty, environment)
        comparisons.append(ProjectComparison(name=project.name, old=old, new=new))
    return sorted(comparisons, key=lambda project: project.name)
