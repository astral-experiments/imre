"""Record the complete pinned pathlib package via CPython's own test runner."""
import argparse
from collections import Counter
import json
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
BENCHMARK = HERE.parents[1]
sys.path.insert(0, str(BENCHMARK))
from upstream import verify_upstream
from suite_inventory import cases, load_suite, reference_results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("run with the test-enabled CPython 3.14.5 interpreter")
    from test import test_pathlib

    commit = verify_upstream()
    pinned = BENCHMARK / "upstream/Lib/test/test_pathlib"
    oracle = Path(test_pathlib.__file__).resolve().parent
    # Compare the complete packages, including helper modules and package hooks.
    def sources(root):
        return {p.relative_to(root): p.read_bytes() for p in root.rglob("*")
                if p.is_file() and "__pycache__" not in p.parts}
    if sources(oracle) != sources(pinned):
        raise ValueError(f"reference package at {oracle} differs from the pinned sources")
    ids = Counter(case.id() for case in cases(load_suite("test.test_pathlib")))

    # Keep AF_UNIX paths short. Enabling EncodingWarning exercises the gated
    # read/write methods; regrtest supplies the original fixtures and assertions.
    with tempfile.TemporaryDirectory(prefix="pl-") as temp:
        xml = Path(temp) / "results.xml"
        subprocess.run([sys.executable, "-B", "-X", "warn_default_encoding",
                        "-m", "test", "-u", "all", "--tempdir", temp,
                        "--junit-xml", str(xml), "test_pathlib"], cwd=temp, check=True)
        records = reference_results(xml, ids)
    report = {"module": "test.test_pathlib", "cpython_commit": commit, "platform": sys.platform,
              "denominator": ids.total(), "reference": records}
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
