"""Discover the enclosing dict suite and run only the requested dependency test."""

import json
from pathlib import Path
import sys
import unittest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[2]))
from suite_inventory import cases, load_suite, verify_files


def reference():
    import test

    manifest = json.loads((HERE / "UPSTREAM.json").read_bytes())
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        raise ValueError("use the pinned CPython 3.14.5")
    verify_files(HERE / "upstream", manifest["files"])
    verify_files(Path(test.__file__).resolve().parents[2],
                 {"Lib/test/test_dict.py": manifest["files"]["Lib/test/test_dict.py"]})
    catalogue = [case.id() for case in cases(load_suite(manifest["module"]))]
    case_id = manifest["selected_id"]
    if catalogue.count(case_id) != 1:
        raise ValueError(f"{case_id} is not a unique upstream occurrence")
    oracle = unittest.TestResult()
    unittest.defaultTestLoader.loadTestsFromName(case_id).run(oracle)
    if oracle.testsRun != 1 or not oracle.wasSuccessful() or oracle.skipped:
        raise ValueError(f"upstream method failed: {oracle.errors} {oracle.failures} {oracle.skipped}")
    return {"module": "collections.dict.fromkeys", "cpython_commit": manifest["commit"],
            "platform": sys.platform, "denominator": 1, "reference": {case_id: "pass"},
            "enclosing_module": manifest["module"], "discovered_in_enclosing_module": catalogue}


if __name__ == "__main__":
    print(json.dumps(reference(), indent=2))
