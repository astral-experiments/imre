"""Run the pinned collections suites, including their load_tests doctests."""

import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_inventory import cases, load_suite, reference_results, unique_ids, verify_files


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--result", type=Path, required=True)
    args = parser.parse_args()
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("use a test-enabled CPython 3.14.5 source build")
    import test

    verify_files(HERE / "upstream", manifest["files"])
    verify_files(Path(test.__file__).resolve().parents[2], manifest["files"])
    modules = []
    discovered = []
    skipped = {}
    for module in manifest["modules"]:
        try:
            suite = load_suite(module)
        except unittest.SkipTest as exc:
            # On this build the parent package raises at import, before the
            # loader can see the sole method in the verified submodule.
            if module != "test.test_free_threading.test_collections" or str(exc) != "GIL enabled":
                raise
            skipped[module + ".TestDeque.test_copy_race"] = {"skipped": str(exc)}
        else:
            discovered.extend(case.id() for case in cases(suite))
            modules.append(module.removeprefix("test."))
    expected = unique_ids(discovered)
    with tempfile.TemporaryDirectory() as work:
        xml = Path(work) / "reference.xml"
        subprocess.run([sys.executable, "-B", "-m", "test", "--single-process",
                        "--tempdir", work, "--randseed", "1", "-u", "cpu",
                        "--junit-xml", xml, *modules], cwd=work, check=True)
        results = reference_results(xml, expected)
    ids = unique_ids([*results, *skipped])
    report = {"cpython_commit": manifest["commit"], "platform": sys.platform,
              "denominator": len(ids), "reference": dict(sorted((results | skipped).items()))}
    args.result.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
