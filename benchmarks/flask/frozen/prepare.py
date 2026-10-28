"""Reconstruct the source fork and compile both registered applications."""

import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
from urllib.parse import urlsplit
from urllib.request import urlopen
from zipfile import ZipFile

HERE = Path(__file__).resolve().parent
BENCH = HERE.parent
ROOT = BENCH.parents[1]


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def verify(actual, expected, label):
    if actual != expected:
        raise RuntimeError(
            f"integrity check failed for {label}: {actual!r} != {expected!r}"
        )


def source_tree(work, wheelhouse, stdlib=ROOT / "stdlib"):
    wheels = work / "wheels"
    site = work / "site"
    wheels.mkdir()
    site.mkdir()
    for requirement in (BENCH / "requirements.txt").read_text().splitlines():
        url, expected = requirement.split(" --hash=sha256:")
        name = Path(urlsplit(url).path).name
        wheel = wheels / name
        if wheelhouse is None:
            with urlopen(url, timeout=60) as response, wheel.open("xb") as local:
                shutil.copyfileobj(response, local)
        else:
            shutil.copyfile(wheelhouse / name, wheel)
        verify(digest(wheel), expected, name)
        with ZipFile(wheel) as archive:
            for member in archive.namelist():
                path = Path(member)
                if path.is_absolute() or ".." in path.parts:
                    raise ValueError(f"invalid wheel member {member!r}")
                if (
                    path.suffix == ".py"
                    or path.name == "py.typed"
                    or (
                        ".dist-info" in path.parts[0]
                        and path.name in {"METADATA", "LICENSE.txt"}
                    )
                ):
                    archive.extract(member, site)

    subprocess.run(
        [
            "patch",
            "--batch",
            "--forward",
            "--fuzz=0",
            "-p1",
            "-i",
            HERE / "upstream.patch",
        ],
        cwd=site,
        check=True,
    )
    # Stage the demo alongside the real imre package, including imre.json.
    shutil.copytree(stdlib / "imre", site / "imre", ignore=shutil.ignore_patterns("__pycache__"))
    for added in (HERE / "overlay").rglob("*.py"):
        target = site / added.relative_to(HERE / "overlay")
        if target.exists():
            raise FileExistsError(target)
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(added, target)
    for name, expected in json.loads((HERE / "fork-files.json").read_bytes()).items():
        verify(digest(site / name), expected, name)
    return site


def python_module(script, paths, *args):
    # Isolate registration and oracle tests from installed third-party packages.
    loader = (
        "import runpy,sys;sys.path[:0]=sys.argv[1:4];sys.argv=sys.argv[4:];"
        "runpy.run_path(sys.argv[0],run_name='__main__')"
    )
    subprocess.run(
        [sys.executable, "-ISB", "-c", loader, *paths, script, *args], check=True
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("work", type=Path, help="new output directory")
    parser.add_argument(
        "--wheelhouse", type=Path, help="read the seven pinned wheels offline"
    )
    parser.add_argument("--imre", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--stdlib", type=Path, default=ROOT / "stdlib")
    args = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5) or sys.implementation.name != "cpython":
        parser.error("the registration and oracle interpreter must be CPython 3.14.5")
    if sys.flags.optimize:
        parser.error("use CPython with assertions enabled")
    work = args.work.resolve()
    work.mkdir(parents=True)
    site = source_tree(work, args.wheelhouse, args.stdlib)
    paths = [site, HERE / "examples", args.stdlib.resolve(strict=True)]
    compiler = args.imre.resolve(strict=True)
    runtime = args.runtime.resolve(strict=True)
    receipt = {
        "compiler_sha256": digest(compiler),
        "runtime_sha256": digest(runtime),
        "python": sys.version.split()[0],
        "graphs": {},
    }
    for name in ("mixed", "catalog"):
        graph = work / name
        python_module(HERE / "examples" / f"freeze_{name}.py", paths, graph)
        sources = {
            p.relative_to(graph).as_posix(): digest(p)
            for p in sorted(graph.rglob("*.py"))
        }
        if name == "mixed":
            verify(
                sources,
                json.loads((HERE / "mixed-graph.json").read_bytes()),
                "mixed graph",
            )
        binaries = {}
        for opt in (0, 3) if name == "mixed" else (3,):
            binary = graph / f"server-O{opt}"
            llvm = graph / f"server-O{opt}.ll"
            subprocess.run(
                [
                    compiler,
                    "build",
                    graph / "serve.py",
                    "--module-root",
                    graph,
                    "--runtime",
                    runtime,
                    "--clang",
                    args.clang,
                    "--no-cache",
                    "--opt-level",
                    str(opt),
                    "--emit-llvm",
                    llvm,
                    "-o",
                    binary,
                ],
                check=True,
            )
            if any(
                "call " in line and "@imre_dyn_" in line
                for line in llvm.read_text().splitlines()
            ):
                raise RuntimeError(
                    f"{name} O{opt} contains dynamic object-dispatch calls"
                )
            binaries[str(opt)] = {
                "elf_sha256": digest(binary),
                "llvm_sha256": digest(llvm),
            }
        receipt["graphs"][name] = {"files": sources, "builds": binaries}
    python_module(
        HERE / "verify_catalog.py", paths, work / "catalog/server-O3", work / "wheels"
    )
    (work / "build.json").write_text(json.dumps(receipt, indent=2) + "\n")


if __name__ == "__main__":
    main()
