"""Print the shared version of the two Relar release artifacts."""

import argparse
import re
import tomllib
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--notes", action="store_true", help="print the reviewed changelog section")
    parser.add_argument("tag", nargs="?", default="dry-run")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    versions = {}
    for name in ("relar-compiler", "relar-runtime"):
        with (root / "crates" / name / "Cargo.toml").open("rb") as manifest:
            versions[name] = tomllib.load(manifest)["package"]["version"]
    if len(set(versions.values())) != 1:
        parser.error(f"release crate versions disagree: {versions}")
    version = versions["relar-compiler"]
    if args.tag not in ("dry-run", version):
        parser.error(f"release tag must be {version!r} or 'dry-run'")
    if not args.notes:
        print(version)
        return

    changelog = root / "CHANGELOG.md"
    if not changelog.is_file():
        parser.error("run the Prepare release workflow and merge its pull request before publishing")
    sections = re.split(r"^## ([^\n]+)\n", changelog.read_text(encoding="utf-8"), flags=re.M)
    notes = [
        body.strip("\n")
        for title, body in zip(sections[1::2], sections[2::2], strict=True)
        if title == version
    ]
    if len(notes) != 1 or not notes[0].strip():
        parser.error(f"expected one nonempty '## {version}' section in CHANGELOG.md")
    print(notes[0])


if __name__ == "__main__":
    main()
