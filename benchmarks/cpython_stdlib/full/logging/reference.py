"""Verify logging's checked-in test and run the complete resource-enabled CPython suite."""

import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
sys.path.insert(0, str(STDLIB))
from suite_inventory import cases, id_counts, load_suite, reference_results, verify_files


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--result", type=Path, required=True)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("run with a test-enabled CPython 3.14.5 build")
    import test

    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(STDLIB / "upstream", manifest["files"])
    verify_files(Path(test.__file__).resolve().parents[2],
                 manifest["files"] | manifest["reference_files"])
    module = manifest["module"]
    ids = id_counts(case.id() for case in cases(load_suite(module)))
    with tempfile.TemporaryDirectory() as work:
        xml = Path(work) / "reference.xml"
        run = subprocess.run([sys.executable, "-B", "-m", "test", "--single-process",
                              "-u", "all", "--tempdir", work, "--randseed", "1",
                              "--junit-xml", xml, module.removeprefix("test.")], cwd=work)
        results = reference_results(xml, ids)
    report = {"module": module, "cpython_commit": manifest["commit"], "platform": sys.platform,
              "resources": "all", "denominator": sum(ids.values()), "reference": results}
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    return run.returncode


if __name__ == "__main__":
    sys.exit(main())
