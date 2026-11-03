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
    p.add_argument("--clang", default="clang-18")
    p.add_argument("--toolchain")
    p.add_argument(
        "--lto-runtime", type=Path, help="bitcode runtime for the road router"
    )
    p.add_argument("--lto-linker", type=Path, help="matching LLD executable")
    args = p.parse_args()
    if bool(args.lto_runtime) != bool(args.lto_linker):
        p.error("--lto-runtime and --lto-linker must be supplied together")
    build = args.build_dir.resolve()
    build.mkdir(parents=True, exist_ok=True)
    target = build / "target"
    prefix = args.imre_prefix.resolve()
    compiler, runtime = prefix / "imre", prefix / "libimre_runtime.a"
    selected = ["+" + args.toolchain] if args.toolchain else []
    cargo = ["cargo", *selected]
    if args.toolchain == "ohm":
        cargo.append("-Zohm-defaults=no")
    env = {**os.environ, "CARGO_TARGET_DIR": str(target)}
    thin_lto = [
        "--config",
        'profile.release.lto="thin"',
        "--config",
        "profile.release.codegen-units=1",
    ]
    for manifest, options in (
        (HERE, ["--bin", "source-search", "--bin", "verify-wheel"]),
        (HERE, ["--bin", "road-routing", *thin_lto]),
        (ROOT / "benchmarks/reference/python_catalog", thin_lto),
        (ROOT / "benchmarks/reference/conformance_report", thin_lto),
    ):
        subprocess.run(
            [
                *cargo,
                "build",
                "--release",
                "--locked",
                "--manifest-path",
                manifest / "Cargo.toml",
                *options,
            ],
            cwd=ROOT,
            env=env,
            check=True,
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
    road_flags = []
    if args.lto_runtime:
        host = subprocess.check_output(
            ["rustc", *selected, "--print", "host-tuple"], text=True
        ).strip()
        road_flags = [
            "--target",
            host,
            "--lto-linker",
            args.lto_linker.resolve(strict=True),
        ]
    for name, (source, root) in programs.items():
        use_lto = name == "roads" and args.lto_runtime
        subprocess.run(
            [
                compiler,
                "build",
                source,
                "--module-root",
                root,
                "--runtime",
                args.lto_runtime.resolve(strict=True) if use_lto else runtime,
                *(road_flags if use_lto else []),
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
        "rustc": version("rustc", *selected, "-Vv"),
        "clang": version(args.clang, "--version").splitlines()[0],
        "sources_sha256": {
            f.relative_to(ROOT).as_posix(): digest(f)
            for f in sorted(
                {
                    *HERE.glob("*.py"),
                    *HERE.glob("*.rs"),
                    HERE / "Cargo.lock",
                    HERE / "Cargo.toml",
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
        "native_options": {
            "imre": "-O3",
            "imre_road_thin_lto": bool(road_flags),
            "rust": "--release",
            "rust_thin_lto_codegen_units_1": [
                "road-routing",
                "python-catalog-reference",
                "conformance-report-reference",
            ],
        },
        "sha256": {str(f.name): digest(f) for f in files},
    }
    if args.lto_runtime:
        metadata["road_lto_runtime_sha256"] = digest(args.lto_runtime)
        metadata["road_lto_linker_sha256"] = digest(args.lto_linker)
    (build / "build.json").write_text(json.dumps(metadata, indent=2) + "\n")


if __name__ == "__main__":
    main()
