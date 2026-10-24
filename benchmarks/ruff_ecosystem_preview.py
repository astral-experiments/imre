"""Capture one pinned Ruff preview comparison for the ecosystem replay benchmark."""

import argparse
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

from ruff_ecosystem_benchmark import HERE, SOURCE, verify

sys.path.insert(0, str(SOURCE.parent))
from replay import diagnostics
from ruff_ecosystem.defaults import DEFAULT_TARGETS

CORPUS = json.loads((HERE / "fixtures/ruff_ecosystem/preview.json").read_text())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--project", choices=[p["name"] for p in CORPUS["projects"]], required=True
    )
    parser.add_argument(
        "--checkout",
        type=Path,
        help="local Git checkout containing the pinned revision",
        required=True,
    )
    for name in ("old-ruff", "new-ruff", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument(
        "--first",
        type=int,
        action="append",
        default=[],
        help="also emit the first N sorted diagnostics per side",
    )
    args = parser.parse_args()
    if any(count <= 0 for count in args.first):
        parser.error("--first requires a positive count")
    project = next(p for p in CORPUS["projects"] if p["name"] == args.project)
    target = next(
        t.with_preview_enabled()
        for t in DEFAULT_TARGETS
        if (t.repo.owner, t.repo.name) == (project["owner"], project["name"])
    )
    args.output.mkdir(parents=True, exist_ok=True)
    row = (
        "\t".join(
            [project["owner"], project["name"], project["revision"], "old", "new"]
        )
        + "\n"
    )
    with tempfile.TemporaryDirectory(prefix="ruff-ecosystem-preview-") as tmp:
        # Ruff's check temporarily rewrites the root config, which can affect its own diagnostics.
        checkout = Path(tmp) / project["name"]
        subprocess.run(
            [
                "git",
                "clone",
                "--quiet",
                "--shared",
                "--no-checkout",
                args.checkout.resolve(),
                checkout,
            ],
            check=True,
        )
        subprocess.run(
            ["git", "checkout", "--quiet", "--detach", project["revision"]],
            cwd=checkout,
            check=True,
        )
        with target.config_overrides.patch_config(checkout, True, {}):
            for label, binary, version, digest in zip(
                ("old", "new"),
                (args.old_ruff, args.new_ruff),
                CORPUS["ruff_versions"],
                project["sha256"],
                strict=True,
            ):
                if (
                    subprocess.check_output(
                        [binary.resolve(), "--version"], text=True
                    ).strip()
                    != f"ruff {version}"
                ):
                    raise ValueError(f"{binary}: expected Ruff {version}")
                path = args.output / label
                with path.open("wb") as out:
                    subprocess.run(
                        [binary.resolve(), *target.check_options.to_ruff_args(), "."],
                        cwd=checkout,
                        env=dict(os.environ, RAYON_NUM_THREADS="1"),
                        stdout=out,
                        check=True,
                    )
                verify(path, digest)
    (args.output / "projects.tsv").write_text(row)
    if args.first:
        streams = {label: diagnostics(args.output / label) for label in ("old", "new")}
        for count in args.first:
            subset = args.output / f"first-{count}"
            subset.mkdir(exist_ok=True)
            for label, lines in streams.items():
                with (subset / label).open("w", encoding="utf-8") as file:
                    file.writelines(line + "\n" for line in lines[:count])
            (subset / "projects.tsv").write_text(row)


if __name__ == "__main__":
    main()
