"""Build one pinned CRC32C core for CPython, Relar, and direct C calls."""

import argparse
import hashlib
import json
import shutil
import subprocess
import sys
import sysconfig
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
EXAMPLE = ROOT / "examples/google_crc32c"
UPSTREAM = EXAMPLE / "upstream"


def run(*command):
    subprocess.run(command, check=True)


def verify_sources(core):
    manifest = json.loads((EXAMPLE / "UPSTREAM.json").read_text())
    if (core / ".git").exists():
        revision = subprocess.check_output(
            ["git", "-C", core, "rev-parse", "HEAD"], text=True
        ).strip()
        if revision != manifest["core"]["commit"]:
            raise ValueError(f"unexpected CRC32C source revision: {revision}")
    for base, files in (
        (UPSTREAM, manifest["python"]["files"]),
        (core, manifest["core"]["files"]),
    ):
        for name, expected in files.items():
            if hashlib.sha256((base / name).read_bytes()).hexdigest() != expected:
                raise ValueError(
                    f"source differs from the pinned release: {base / name}"
                )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--crc32c-source", required=True, type=Path, help="google/crc32c 1.1.2 checkout"
    )
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument(
        "--relar-include", type=Path, help="directory containing Relar's Python.h"
    )
    parser.add_argument("--cc", default="cc")
    parser.add_argument("--cxx", default="c++")
    args = parser.parse_args()
    output = args.output.resolve()
    core = args.crc32c_source.resolve()
    verify_sources(core)
    build = output / "core-build"
    run(
        "cmake",
        "-S",
        core,
        "-B",
        build,
        "-G",
        "Ninja",
        "-DCMAKE_BUILD_TYPE=Release",
        f"-DCMAKE_C_COMPILER={args.cc}",
        f"-DCMAKE_CXX_COMPILER={args.cxx}",
        "-DBUILD_SHARED_LIBS=OFF",
        "-DCMAKE_POSITION_INDEPENDENT_CODE=ON",
        "-DCRC32C_BUILD_TESTS=OFF",
        "-DCRC32C_BUILD_BENCHMARKS=OFF",
        "-DCRC32C_INSTALL=OFF",
        "-DCRC32C_USE_GLOG=OFF",
        "-DCMAKE_POLICY_VERSION_MINIMUM=3.5",
    )
    run("cmake", "--build", build, "--target", "crc32c")
    archive = build / "libcrc32c.a"
    extension = UPSTREAM / "_crc32c.c"
    standard_library = "c++" if sys.platform == "darwin" else "stdc++"
    cc = [args.cc, "-O3", "-fPIC", "-I", core / "include"]

    host = output / "cpython/google_crc32c"
    shutil.copytree(UPSTREAM / "google_crc32c", host, dirs_exist_ok=True)
    suffix = sysconfig.get_config_var("EXT_SUFFIX")
    host_link = (
        ["-bundle", "-undefined", "dynamic_lookup"]
        if sys.platform == "darwin"
        else ["-shared"]
    )
    run(
        *cc,
        *host_link,
        "-I",
        sysconfig.get_path("include"),
        extension,
        archive,
        f"-l{standard_library}",
        "-o",
        host / f"_crc32c{suffix}",
    )
    run(
        *cc,
        ROOT / "benchmarks/google_crc32c.c",
        archive,
        f"-l{standard_library}",
        "-o",
        output / "direct",
    )

    if args.relar_include:
        stage = output / "wheel/google_crc32c"
        shutil.copytree(EXAMPLE / "wheel/google_crc32c", stage, dirs_exist_ok=True)
        licenses = stage / "licenses"
        licenses.mkdir(exist_ok=True)
        shutil.copyfile(UPSTREAM / "LICENSE", licenses / "google-crc32c.txt")
        shutil.copyfile(core / "LICENSE", licenses / "crc32c.txt")
        obj = output / "relar-crc32c.o"
        run(*cc, "-I", args.relar_include.resolve(), "-c", extension, "-o", obj)
        target = stage / "_crc32c.relar.a"
        shutil.copyfile(archive, target)
        run("ar", "rcs", target, obj)
        (stage / "_crc32c.relar.libs").write_text(standard_library + "\n")


if __name__ == "__main__":
    main()
