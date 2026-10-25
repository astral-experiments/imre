"""Pack a staged Imre extension using the standard wheel format."""

import argparse
import sys
import sysconfig
from pathlib import Path

from packaging.tags import mac_platforms
from packaging.utils import canonicalize_name
from packaging.version import Version
from wheel.wheelfile import WheelFile


def host_platform() -> str:
    if sys.platform == "darwin":
        return next(mac_platforms())
    return sysconfig.get_platform().replace("-", "_").replace(".", "_")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("stage", type=Path, help="root of the package payload")
    parser.add_argument("--name", required=True)
    parser.add_argument("--version", required=True, type=Version)
    parser.add_argument("--platform", default=host_platform())
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    name = canonicalize_name(args.name, validate=True)
    if not all(part.isalnum() for part in args.platform.split("_")):
        parser.error("expected a wheel platform tag such as linux_x86_64")
    if args.output.resolve().is_relative_to(args.stage.resolve()):
        parser.error("the output directory must be outside the staged package")
    archives = list(args.stage.rglob("*.imre.a"))
    if not archives:
        parser.error("the package contains no .imre.a archive")
    for archive in archives:
        stub = archive.with_name(archive.name.removesuffix(".imre.a") + ".pyi")
        if not stub.is_file():
            parser.error(f"missing native declarations at {stub}")

    dist = name.replace("-", "_")
    info = f"{dist}-{args.version}.dist-info"
    tag = f"im314-imre1-{args.platform}"
    args.output.mkdir(parents=True, exist_ok=True)
    output = args.output / f"{dist}-{args.version}-{tag}.whl"
    with WheelFile(str(output), "w") as wheel:
        for path in sorted(args.stage.rglob("*")):
            if path.is_file():
                wheel.write(path, path.relative_to(args.stage).as_posix())
        wheel.writestr(
            f"{info}/METADATA",
            f"Metadata-Version: 2.1\nName: {name}\nVersion: {args.version}\n",
        )
        wheel.writestr(
            f"{info}/WHEEL",
            f"Wheel-Version: 1.0\nGenerator: imre\nRoot-Is-Purelib: false\nTag: {tag}\n",
        )
    print(output)


if __name__ == "__main__":
    main()
