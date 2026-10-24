"""Collect reports from already prepared project trees with native subprocesses."""

import os
import subprocess
import tempfile
from dataclasses import dataclass
from pathlib import Path

from imre.json import decode

from .model import MemoryReport, ProjectComparison
from .publish import write_capture


@dataclass(slots=True)
class PreparedProject:
    name: str
    cwd: str
    argv: list[str]


@dataclass(slots=True)
class PreparedProjects:
    projects: list[PreparedProject]


def collect_report(
    project: PreparedProject, executable: str, environment: dict[str, str],
    capture: Path | None, workspace: Path | None, role: str,
) -> MemoryReport:
    command = [executable if argument == "{ty}" else argument for argument in project.argv]
    if capture is None:
        result = subprocess.run(
            command, cwd=project.cwd, env=environment, capture_output=True,
            text=True, encoding="utf-8", check=True,
        )
    else:
        if workspace is None:
            raise ValueError("capture workspace is missing")
        result = subprocess.run(
            command, cwd=project.cwd, env=environment, capture_output=True,
            text=True, encoding="utf-8", check=False,
        )
        write_capture(workspace, capture, role, result.stdout, result.stderr)
        result.check_returncode()
    return decode(result.stdout, MemoryReport)


def collect_projects(
    manifest: PreparedProjects, old_ty: str, new_ty: str, environment: dict[str, str],
    capture_directory: Path | None, workspace: Path | None,
) -> list[ProjectComparison]:
    comparisons: list[ProjectComparison] = []
    for index, project in enumerate(manifest.projects):
        capture: Path | None = None
        if capture_directory is not None:
            capture = capture_directory / ("project-" + str(index))
            capture.mkdir()
        old = collect_report(project, old_ty, environment, capture, workspace, "old")
        new = collect_report(project, new_ty, environment, capture, workspace, "new")
        comparisons.append(ProjectComparison(name=project.name, old=old, new=new))
    return sorted(comparisons, key=lambda project: project.name)


def run_prepared(path: Path, old_ty: str, new_ty: str, capture_directory: str | None) -> list[ProjectComparison]:
    if old_ty[:1] != "/" or new_ty[:1] != "/":
        raise ValueError("--old-ty and --new-ty must be absolute executable paths")
    manifest_text = path.read_text(encoding="utf-8")
    manifest = decode(manifest_text, PreparedProjects)
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
    if capture_directory is None:
        return collect_projects(manifest, old_ty, new_ty, environment, None, None)
    capture = Path(capture_directory)
    capture.mkdir()
    with tempfile.TemporaryDirectory(dir=capture, prefix=".imre-capture-") as name:
        workspace = Path(name)
        staged_manifest = workspace / "projects.json"
        staged_manifest.write_text(manifest_text, encoding="utf-8")
        staged_manifest.replace(capture / "projects.json")
        comparisons = collect_projects(manifest, old_ty, new_ty, environment, capture, workspace)
    return comparisons
