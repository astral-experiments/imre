"""Build the native release's boxed stdlib archives using its bundled Clang."""

import argparse
import os
import shutil
import subprocess
import tempfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path


def instrument(source):
    lines = source.read_text().splitlines(keepends=True)
    definitions = 0
    for index, line in enumerate(lines):
        if line.startswith("define "):
            if not line.rstrip().endswith("{"):
                raise ValueError(f"unexpected LLVM function definition in {source}")
            lines[index] = line.rsplit("{", 1)[0] + "sanitize_address {\n"
            definitions += 1
    if not definitions:
        raise ValueError(f"provider has no LLVM function definitions: {source}")
    source.write_text("".join(lines))


def build(compiler, levels, clang=None, sanitize=False):
    # The Linux release runners execute the compiler and Clang on the target architecture.
    compiler = compiler.absolute()
    toolchain = compiler.parent / "imre-toolchain"
    clang = clang or toolchain / "bin" / "clang"
    archiver = shutil.which("ar") or shutil.which("llvm-ar")
    if not archiver:
        raise FileNotFoundError("building bundled providers needs GNU ar or llvm-ar")
    output = toolchain / "lib" / "imre-stdlib"
    if sanitize:
        output /= "asan"
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=output) as temp:
        stage = Path(temp)
        ir = stage / "ir"
        subprocess.run([compiler, "--emit-bundled-provider-ir", ir], check=True)
        modules = sorted(ir.glob("imre_provider_*.ll"))
        if not modules:
            raise ValueError("compiler emitted no bundled providers")
        if sanitize:
            for source in modules:
                instrument(source)
        # Publish each level before starting the next, then remove its objects.
        for level in levels:
            objects = stage / level
            objects.mkdir()

            def compile_module(source):
                obj = objects / source.with_suffix(".o").name
                flags = ["-fsanitize=address", "-fno-omit-frame-pointer"] if sanitize else []
                subprocess.run([clang, "--no-default-config", f"-{level}", *flags, "-Wno-override-module",
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
    parser.add_argument("--opt-level", choices=("0", "1", "3"), help="build only this archive")
    parser.add_argument("--clang", type=Path, help="use this Clang instead of the bundled driver")
    parser.add_argument("--sanitize", action="store_true", help="build instrumented O0/O1 archives for AddressSanitizer")
    args = parser.parse_args()
    levels = ["O" + args.opt_level] if args.opt_level else (["O0", "O1"] if args.sanitize else ["O0", "O3"])
    build(args.compiler, levels, args.clang, args.sanitize)
