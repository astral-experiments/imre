"""Build the CLI benchmarks before moving the host into the timing window."""

import argparse
import hashlib
import json
import os
import shutil
import subprocess
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--build-dir", required=True, type=Path)
    p.add_argument("--imre-prefix", required=True, type=Path)
    p.add_argument("--bun", default="bun")
    p.add_argument("--clang", default="clang-18")
    p.add_argument("--toolchain")
    args = p.parse_args()
    build = args.build_dir.resolve()
    build.mkdir(parents=True, exist_ok=True)
    target = build / "target"
    prefix = args.imre_prefix.resolve()
    compiler, runtime = prefix / "imre", prefix / "libimre_runtime.a"
    if not (bun := shutil.which(args.bun)):
        p.error(f"Bun executable not found: {args.bun}")
    bun = Path(bun).resolve()
    selected = ["+" + args.toolchain] if args.toolchain else []
    cargo = ["cargo", *selected]
    if args.toolchain == "ohm":
        cargo.append("-Zohm-defaults=no")
    env = {**os.environ, "CARGO_TARGET_DIR": str(target)}
    for manifest in (
        HERE,
        ROOT / "benchmarks/reference/python_catalog",
        ROOT / "benchmarks/reference/conformance_report",
    ):
        subprocess.run(
            [
                *cargo,
                "build",
                "--release",
                "--locked",
                "--manifest-path",
                manifest / "Cargo.toml",
            ],
            cwd=ROOT,
            env=env,
            check=True,
        )
    subprocess.run(
        [bun, "install", "--frozen-lockfile", "--ignore-scripts"], cwd=HERE, check=True
    )
    wheel_src = build / "wheel-src"
    wheel = ROOT / "examples/wheel_record"
    shutil.copytree(wheel / "upstream", wheel_src, dirs_exist_ok=True)
    shutil.copy2(wheel / "validate.py", wheel_src)
    examples = ROOT / "examples"
    programs = {
        "catalog": (examples / "python_catalog/__main__.py", examples),
        "search": (HERE / "search.py", HERE),
        "wheel": (wheel_src / "validate.py", wheel_src),
        "roads": (examples / "road_routing.py", examples),
        "conformance": (examples / "conformance_report/__main__.py", examples),
    }
    for name, (source, root) in programs.items():
        subprocess.run(
            [
                compiler,
                "build",
                source,
                "--module-root",
                root,
                "--runtime",
                runtime,
                "--clang",
                args.clang,
                "--opt-level",
                "3",
                "--reachable-only",
                "--no-cache",
                "-o",
                build / name,
            ],
            cwd=ROOT,
            check=True,
        )

    def version(*cmd):
        return subprocess.check_output(cmd, text=True, cwd=ROOT).strip()

    def digest(path):
        return hashlib.sha256(path.read_bytes()).hexdigest()

    files = [
        compiler,
        runtime,
        bun,
        *(build / n for n in programs),
        *(
            target / "release" / n
            for n in [
                "road-routing",
                "verify-wheel",
                "source-search",
                "python-catalog-reference",
                "conformance-report-reference",
            ]
        ),
    ]
    metadata = {
        "imre": version(compiler, "--version"),
        "bun": version(bun, "--revision"),
        "rustc": version("rustc", *selected, "-Vv"),
        "clang": version(args.clang, "--version").splitlines()[0],
        "sources_sha256": {
            f.relative_to(ROOT).as_posix(): digest(f)
            for f in sorted(
                {
                    *HERE.glob("*.py"),
                    *HERE.glob("*.rs"),
                    *HERE.glob("*.mjs"),
                    HERE / "Cargo.lock",
                    HERE / "Cargo.toml",
                    HERE / "bun.lock",
                    HERE / "package.json",
                    *ROOT.glob("benchmarks/reference/python_catalog/src/*.rs"),
                    *ROOT.glob("benchmarks/reference/conformance_report/src/*.rs"),
                    *ROOT.glob("examples/conformance_report/*.py"),
                    *ROOT.glob("examples/python_catalog/*.py"),
                    *ROOT.glob("examples/wheel_record/upstream/**/*.py"),
                    examples / "wheel_record/validate.py",
                    examples / "road_routing.py",
                    examples / "route_planner.py",
                }
            )
        },
        "native_options": "Imre O3 and Cargo release --locked (Ohm experimental defaults off when selected)",
        "sha256": {str(f.name): digest(f) for f in files},
    }
    (build / "build.json").write_text(json.dumps(metadata, indent=2) + "\n")


if __name__ == "__main__":
    main()
