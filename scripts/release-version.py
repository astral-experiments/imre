"""Print the shared version of the two Relar release artifacts."""

import argparse
import tomllib
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tag", nargs="?", default="dry-run")
    tag = parser.parse_args().tag
    root = Path(__file__).resolve().parents[1]
    versions = {}
    for name in ("relar-compiler", "relar-runtime"):
        with (root / "crates" / name / "Cargo.toml").open("rb") as manifest:
            versions[name] = tomllib.load(manifest)["package"]["version"]
    if len(set(versions.values())) != 1:
        parser.error(f"release crate versions disagree: {versions}")
    version = versions["relar-compiler"]
    if tag not in ("dry-run", version):
        parser.error(f"release tag must be {version!r} or 'dry-run'")
    print(version)


if __name__ == "__main__":
    main()
