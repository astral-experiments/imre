"""Compare the unchanged upstream wheel checker on CPython and native Imre."""

import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from zipfile import ZIP_DEFLATED, ZipFile


UPSTREAM = Path(__file__).resolve().parents[1] / "examples/uv_wheel_check/upstream"
SOURCE = UPSTREAM / "check_uv_wheel_contents.py"


def members(package, windows=False):
    """Independently construct small fixtures with the 0.13.1 release layout."""
    prefix = f"{package}-0.13.1"
    command = package.replace("_", "-")
    names = [f"{package}/{name}" for name in ("__init__.py", "__main__.py", "py.typed")]
    commands = [command]
    if package == "uv":
        names.append("uv/_find_uv.py")
        commands.append("uvx")
        if windows:
            commands.append("uvw")
    for name in commands:
        names.append(f"{prefix}.data/scripts/{name}" + (".exe" if windows else ""))
    for name in ("METADATA", "RECORD", "WHEEL", "licenses/LICENSE-APACHE", "licenses/LICENSE-MIT",
                 f"sboms/{command}.cyclonedx.json"):
        names.append(f"{prefix}.dist-info/{name}")
    return names


def wheel(root, filename, names):
    with ZipFile(root / filename, "w", compression=ZIP_DEFLATED) as archive:
        for name in names:
            archive.writestr(name, b"fixture\n")
    return filename


def run(command, cwd):
    result = subprocess.run(command, cwd=cwd, capture_output=True, timeout=60)
    return result.returncode, result.stdout, result.stderr


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--opt-levels", nargs="+", choices=("0", "1", "2", "3"), default=("0", "3"))
    parser.add_argument("--wheels", nargs="+", type=Path, help="also check these real wheels as a batch")
    args = parser.parse_args()
    for filename, digest in json.loads((UPSTREAM / "UPSTREAM.json").read_bytes())["files"].items():
        if hashlib.sha256((UPSTREAM / filename).read_bytes()).hexdigest() != digest:
            raise RuntimeError(f"modified upstream file: {filename}")
    with tempfile.TemporaryDirectory(prefix="imre-uv-wheels-") as temporary:
        root = Path(temporary)
        uv = members("uv")
        linux = wheel(root, "uv-0.13.1-linux.whl", uv)
        win = wheel(root, "uv-0.13.1-win_amd64.whl", members("uv", windows=True))
        build = wheel(root, "uv_build-0.13.1-linux.whl", members("uv_build"))
        missing = wheel(root, "uv-0.13.1-missing.whl", [n for n in uv if n != "uv/py.typed"])
        extra = wheel(root, "uv-0.13.1-extra.whl", [*uv, "é/unexpected'file"])
        # A singleton in each set makes both error messages byte-comparable
        # without depending on a particular Python set iteration order.
        both = wheel(root, "uv-0.13.1-both.whl", [n for n in uv if n != "uv/__main__.py"] + ["🦀"])
        cases = [
            ([linux], 0), ([win], 0), ([build], 0), (["skip.tar.gz", linux, build, win], 0),
            (["skip.tar.gz"], 0), (["--", linux], 0), ([missing], 1), ([extra], 1),
            ([both], 1), ([linux, missing, win], 1),
        ]
        if args.wheels:
            cases.append(([str(path.resolve()) for path in args.wheels], 0))
        expected = []
        for argv, status in cases:
            result = run([args.python, "-I", "-B", str(SOURCE), *argv], root)
            if result[0] != status:
                raise AssertionError(("CPython", argv, result))
            expected.append(result)
        for level in args.opt_levels:
            binary = root / ("checker-O" + level)
            subprocess.run([args.compiler.resolve(), "build", SOURCE, "--no-cache", "--quiet",
                            "--runtime", args.runtime.resolve(), "--clang", args.clang,
                            "--opt-level", level, "-o", binary], cwd=root, check=True, timeout=120)
            for (argv, _), reference in zip(cases, expected, strict=True):
                actual = run([binary, *argv], root)
                if actual != reference:
                    raise AssertionError((level, argv, actual, reference))
            print(f"O{level}: {len(cases)} matched exit status, stdout, and stderr")


if __name__ == "__main__":
    main()
