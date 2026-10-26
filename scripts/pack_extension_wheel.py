"""Pack a staged Imre extension using the standard wheel format."""

import argparse
import json
import subprocess
from pathlib import Path

from packaging.utils import canonicalize_name
from packaging.version import Version
from wheel.wheelfile import WheelFile


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("stage", type=Path, help="root of the package payload")
    parser.add_argument("--name", required=True)
    parser.add_argument("--version", required=True, type=Version)
    parser.add_argument("--imre", default="imre", help="target Imre compiler")
    parser.add_argument("--platform", help="override the native platform for a staged archive")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    name = canonicalize_name(args.name, validate=True)
    if args.platform and not all(part.isalnum() for part in args.platform.split("_")):
        parser.error("expected a wheel platform tag such as linux_x86_64")
    if args.output.resolve().is_relative_to(args.stage.resolve()):
        parser.error("the output directory must be outside the staged package")
    target = json.loads(subprocess.check_output([args.imre, "build-target"]))
    suffix = target["archive_suffix"]
    archives = list(args.stage.rglob(f"*{suffix}"))
    if not archives:
        parser.error(f"the package contains no {suffix} archive")
    for archive in archives:
        stub = archive.with_name(archive.name.removesuffix(suffix) + ".pyi")
        if not stub.is_file():
            parser.error(f"missing native declarations at {stub}")

    dist = name.replace("-", "_")
    info = f"{dist}-{args.version}.dist-info"
    platform = args.platform or target["platform_tag"]
    tag = f"{target['python_tag']}-{target['abi_tag']}-{platform}"
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
