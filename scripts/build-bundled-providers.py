"""Build the native release's boxed stdlib archives using its bundled Clang."""

import argparse
import os
import shutil
import subprocess
import tempfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path


def build(compiler, levels):
    # The Linux release runners execute the compiler and Clang on the target architecture.
    compiler = compiler.absolute()
    toolchain = compiler.parent / "imre-toolchain"
    clang = toolchain / "bin" / "clang"
    archiver = shutil.which("ar") or shutil.which("llvm-ar")
    if not archiver:
        raise FileNotFoundError("building bundled providers needs GNU ar or llvm-ar")
    output = toolchain / "lib" / "imre-stdlib"
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=output) as temp:
        stage = Path(temp)
        ir = stage / "ir"
        subprocess.run([compiler, "--emit-bundled-provider-ir", ir], check=True)
        modules = sorted(ir.glob("imre_provider_*.ll"))
        if not modules:
            raise ValueError("compiler emitted no bundled providers")
        # Publish O0 before building O3, and remove each level's objects after archiving.
        for level in levels:
            objects = stage / level
            objects.mkdir()

            def compile_module(source):
                obj = objects / source.with_suffix(".o").name
                subprocess.run([clang, "--no-default-config", f"-{level}", "-Wno-override-module",
                                "-c", "-x", "ir", source, "-o", obj], check=True)
                if level == levels[-1]:
                    source.unlink()

            with ThreadPoolExecutor(max_workers=min(4, len(os.sched_getaffinity(0)))) as pool:
                list(pool.map(compile_module, modules))

            # Pass member basenames to avoid path-dependent archives or long command lines.
            subprocess.run([archiver, "crsD", "providers.a", *(s.with_suffix(".o").name for s in modules)],
                           cwd=objects, check=True)
            destination = output / level
            destination.mkdir(exist_ok=True)
            (objects / "providers.a").replace(destination / "providers.a")
            shutil.rmtree(objects)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("compiler", type=Path, help="native imre executable beside imre-toolchain/")
    parser.add_argument("--opt-level", choices=("0", "3"), help="build only this archive (default: both)")
    args = parser.parse_args()
    build(args.compiler, ["O" + args.opt_level] if args.opt_level else ["O0", "O3"])
