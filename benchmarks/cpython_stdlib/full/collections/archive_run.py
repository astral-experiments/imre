"""Archive one complete collections result from a downloaded hosted run."""

import argparse
import json
from pathlib import Path
import shutil
import subprocess
import sys

HERE = Path(__file__).resolve().parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("download", type=Path, help="directory containing source.json and the O0/O3 results")
    args = parser.parse_args()
    src = args.download
    origin = json.loads((src / "source.json").read_bytes())
    revision = origin["compiler_revision"]
    tree, base, head = subprocess.check_output(
        ["git", "show", "-s", "--format=%T %P", revision], text=True).split()
    if (tree, base, head) != (origin["tree"], origin["base"], origin["head"]):
        parser.error("the downloaded source does not match the verifier's Git commit")
    evidence = src / "evidence-linux.json"
    subprocess.run([sys.executable, HERE.parents[1] / "suite_evidence.py",
                    HERE / "reference-linux.json", evidence], check=True)
    reports = json.loads(evidence.read_bytes())
    if [report["opt_level"] for report in reports] != [0, 3]:
        parser.error("expected separate complete O0 and O3 reports")
    for report in reports:
        if (report["cpython_commit"] != origin["cpython_commit"]
                or report["denominator"] != 659
                or any(row["evidence"].get("compiler_revision") != revision for row in report["records"]
                       if row["classification"] != "reference-skip")):
            parser.error("source or occurrence mismatch in evidence")

    dest = HERE / "runs" / revision[:12]
    dest.mkdir(parents=True, exist_ok=True)
    files = [evidence, src / "source.json", src / "fixtures-O0.json", src / "fixtures-O3.json"]
    files.extend(sorted(src.glob("*-controls-O[03].json")))
    for file in files:
        target = dest / file.name
        if target.exists() and target.read_bytes() != file.read_bytes():
            parser.error(f"existing receipt differs: {target}")
        shutil.copyfile(file, target)


if __name__ == "__main__":
    main()
