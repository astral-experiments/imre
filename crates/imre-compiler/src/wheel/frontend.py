"""Build a wheel with the project's setuptools backend and Imre's native target."""

import argparse
import json
import os
import runpy
import shutil
import subprocess
import sys
from contextlib import nullcontext
from functools import partial
from pathlib import Path
from zipfile import ZipFile

TOOLS = Path(__file__).resolve().parent


def copy_project(source, dest, out_dir):
    """Start with clean setuptools outputs, retaining Git version discovery."""

    def ignore(directory, names):
        here = Path(directory)
        return {
            name
            for name in names
            if name == "__pycache__"
            or name.endswith(".egg-info")
            or (
                here == source
                and name in {".git", ".venv", ".tox", ".nox", "build", "dist"}
            )
            or here / name in {TOOLS, out_dir}
        }

    shutil.copytree(source, dest, ignore=ignore, symlinks=True)
    git = source / ".git"
    if git.exists():
        if git.is_file():
            git = source / git.read_text().strip().removeprefix("gitdir: ")
        (dest / ".git").write_text(f"gitdir: {git.resolve()}\n")


def build_wheel(args):
    import tomllib
    from build import ProjectBuilder
    from build.env import DefaultIsolatedEnv

    source = args.project.resolve()
    project = source / "pyproject.toml"
    with project.open("rb") as stream:
        system = tomllib.load(stream).get("build-system", {})
    backend = system.get("build-backend", "setuptools.build_meta:__legacy__")
    if backend not in {
        "setuptools.build_meta",
        "setuptools.build_meta:__legacy__",
    } or system.get("backend-path"):
        raise ValueError(
            f"imre wheel supports the setuptools.build_meta backend; found {backend!r} in {project}"
        )

    overlay = args.overlay.resolve(strict=True) if args.overlay else None
    if overlay and not overlay.is_dir():
        raise ValueError(f"wheel overlay must be a directory: {overlay}")
    out_dir = (args.out_dir or source / "dist").resolve()
    staged = TOOLS / "project"
    copy_project(source, staged, out_dir)
    settings = json.dumps([str(overlay) if overlay else None, args.system_library])

    def run_hook(cmd, cwd, extra_environ):
        # Use PyPA's normal hook subprocess and environment. Retarget setuptools
        # before it imports setup.py (projects can subclass its commands there).
        subprocess.run(
            [cmd[0], __file__, "--backend-hook", settings, *cmd[1:]],
            cwd=cwd,
            env={**os.environ, **extra_environ},
            check=True,
        )

    with nullcontext() if args.no_build_isolation else DefaultIsolatedEnv() as env:
        if env is None:
            make_builder = partial(ProjectBuilder, staged)
            missing = make_builder().check_dependencies("wheel")
            if missing:
                raise ValueError(
                    "missing build dependencies: "
                    + ", ".join(sorted(req[0] for req in missing))
                )
        else:
            make_builder = partial(ProjectBuilder.from_isolated_env, env, staged)
            builder = make_builder()
            env.install(builder.build_system_requires)
            env.install(builder.get_requires_for_build("wheel"))
        wheel = Path(make_builder(runner=run_hook).build("wheel", TOOLS / "wheelhouse"))

    # A setup.py may swallow a compiler error and successfully build a pure
    # wheel. Only deliver an artifact for this target after the hook succeeds.
    from packaging.utils import parse_wheel_filename

    tags = parse_wheel_filename(wheel.name)[3]
    with ZipFile(wheel) as archive:
        if any(
            (tag.interpreter, tag.abi) != ("im314", "imre1") for tag in tags
        ) or not any(name.endswith(".imre.a") for name in archive.namelist()):
            raise ValueError(
                f"setuptools produced {wheel.name} without an Imre native extension"
            )
    out_dir.mkdir(parents=True, exist_ok=True)
    print(shutil.copy2(wheel, out_dir), flush=True)


def main():
    parser = argparse.ArgumentParser(
        prog="imre wheel", description=__doc__, allow_abbrev=False
    )
    parser.add_argument(
        "project", type=Path, help="directory containing pyproject.toml"
    )
    parser.add_argument(
        "--out-dir", type=Path, help="wheel directory (default: PROJECT/dist)"
    )
    parser.add_argument(
        "--overlay", type=Path, help="wheel-relative type stubs and Python wrappers"
    )
    parser.add_argument(
        "--system-library",
        action="append",
        default=[],
        help="library to link when building the application (repeatable)",
    )
    parser.add_argument(
        "--no-build-isolation",
        action="store_true",
        help="use build dependencies installed in the host Python",
    )
    parser.add_argument(
        "--python", help="host Python with the build package (default: python3)"
    )
    args = parser.parse_args()
    if sys.version_info < (3, 11):
        parser.exit(
            1,
            "imre wheel requires a build host with Python 3.11 or newer; pass --python to select it\n",
        )
    try:
        import build
    except ModuleNotFoundError:
        parser.exit(
            1,
            f"imre wheel requires the PyPA build frontend. Install it with {sys.executable} -m pip install build\n",
        )
    try:
        build_wheel(args)
    except (
        OSError,
        ValueError,
        build.BuildException,
        build.BuildBackendException,
    ) as error:
        parser.exit(1, f"imre wheel: {error}\n")


if __name__ == "__main__":
    if sys.argv[1:2] == ["--backend-hook"]:
        overlay, libraries = json.loads(sys.argv[2])
        install_target = runpy.run_path(str(TOOLS / "setuptools_target.py"))[
            "install_target"
        ]
        install_target(TOOLS, overlay, libraries)
        sys.argv = sys.argv[3:]
        runpy.run_path(sys.argv[0], run_name="__main__")
    else:
        main()
