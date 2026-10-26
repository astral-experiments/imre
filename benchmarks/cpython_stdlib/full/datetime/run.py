"""Compile unchanged datetime methods under CPython's full default suite."""

import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import roles, write_reports
from suite_inventory import (
    discovery_resources,
    load_suite,
    verify_files,
    verify_interpreter_tests,
)
from suite_native import NativeCompileError, Toolchain, method_bytes, run_evidence

CANDIDATES = {
    "TestDateOnly": ["test_delta_non_days_ignored"],
    "TestModule": ["test_utc_alias"],
    "TestTimeDelta": [
        "test_basic_attributes",
        "test_carries",
        "test_bool",
        "test_massive_normalization",
    ],
    "TestTimeZone": ["test_str", "test_class_members"],
}
ZONE_LAYOUT = b"""    ACDT: timezone
    EST: timezone
    DT: datetime
    def __init__(self):
        super().__init__()
        self.ACDT = timezone.utc
        self.EST = timezone.utc
        self.DT = datetime(2000, 1, 1)
"""
SOURCE = HERE / "upstream/Lib/test/datetimetester.py"


def prepare(tools, directory, owner, name):
    original = method_bytes(SOURCE, owner, name)
    zone = owner == "TestTimeZone"
    fixture = ZONE_LAYOUT + method_bytes(SOURCE, owner, "setUp") if zone else b""
    source = (
        b"from datetime import date, datetime, timedelta, timezone, UTC\nimport unittest\n"
        b"ZERO = timedelta(0)\nclass Case(unittest.TestCase):\n"
        + fixture
        + original
        + b"\nc = Case()\n"
        + (b"c.setUp()\n" if zone else b"")
        + f"c.{name}()\nprint('completed')\n".encode()
    )
    proof = {
        "method_sha256": hashlib.sha256(original).hexdigest(),
        "opt_level": tools.level,
    }
    try:
        program = tools.build(source, directory)
    except NativeCompileError as e:
        diagnostic = str(e).rsplit("Imre: ", 1)
        if len(diagnostic) != 2:
            raise  # Never treat an infrastructure or linker failure as an unsupported method.
        return None, {
            "outcome": "blocked",
            "reason": diagnostic[1].strip(),
            "roles": roles(
                body="unobserved",
                assertions="unobserved",
                operation="unobserved",
                fixture="unobserved",
            ),
            "evidence": {**proof, "stage": "compile"},
        }

    def execute(case, original_method):
        actual = subprocess.run([program], capture_output=True, timeout=15)
        if (actual.returncode, actual.stdout, actual.stderr) != (
            0,
            b"completed\n",
            b"",
        ):
            raise AssertionError((case.id(), actual))
        return {
            "roles": roles(
                body="imre",
                assertions="imre",
                operation="imre",
                fixture="mixed" if zone else "cpython",
            ),
            "evidence": {
                **proof,
                "operation": "Native datetime values and native unittest assertions",
                "fixture": (
                    "The original native setUp assigns three declared fields; CPython runs its upstream Pure/Fast import and TestCase hooks."
                    if zone
                    else "CPython runs its upstream Pure/Fast import and TestCase hooks."
                ),
            },
        }

    return execute, None


def upstream_skips(reference, level):
    exceptions = {}
    for case_id, value in reference["reference"].items():
        if not isinstance(value, dict) or "skipped" not in value:
            continue
        fixture, method = case_id.rsplit(".", 1)
        cls = fixture.rsplit(".", 1)[-1]
        if cls in {"CapiTest_Pure", "ExtensionModuleTests_Pure"}:
            condition = "The upstream C API or extension setUp skips the pure Python implementation."
            line = 6785 if cls == "CapiTest_Pure" else 7219
        elif cls.startswith("TestLocalTimeDisambiguation_") and method in {
            "test_fromtimestamp_lord_howe",
            "test_timestamp_lord_howe",
        }:
            condition = "Under Australia/Lord_Howe the system %Z%z spelling at 1.4e9 differs from LHST+1030."
            line = 6248 if method == "test_fromtimestamp_lord_howe" else 6276
        elif cls == "TestModule_Fast" and method == "test_divide_and_round":
            condition, line = "The original helper exists only in _pydatetime.", 107
        elif cls == "TestModule_Pure" and method == "test_name_cleanup":
            condition, line = (
                "The original method inspects only the accelerated module.",
                95,
            )
        elif cls.startswith("TestSubclassDateTime_") and method == "test_roundtrip":
            condition, line = (
                "The upstream subclass explicitly skips the inherited roundtrip test.",
                3653,
            )
        else:
            raise ValueError(f"Unreviewed original reference skip: {case_id}: {value}")
        exceptions[(case_id, 1, level)] = {
            "kind": "upstream-skip",
            "platform": "linux",
            "fixture_variant": fixture,
            "configuration": "CPython 3.14.5 default regrtest resources",
            "reason": value["skipped"],
            "condition": condition,
            "source": f"Lib/test/datetimetester.py:{line}",
        }
    return exceptions


def main():
    parser = argparse.ArgumentParser()
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--cpython-source", type=Path)
    args = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5):
        parser.error("the pinned CPython 3.14.5 interpreter is required")
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    spec = json.loads((HERE / "UPSTREAM.json").read_text())
    ref = json.loads((HERE / "reference-linux.json").read_text())
    verify_files(HERE / "upstream", spec["files"])
    verify_interpreter_tests(spec)
    reports = []
    with discovery_resources(spec):
        for level in (0, 3):
            tools = Toolchain(args.imre, args.runtime, args.clang, level)
            with tempfile.TemporaryDirectory() as tmp:
                executions, preflight = {}, {}
                for owner, methods in CANDIDATES.items():
                    for name in methods:
                        work = Path(tmp) / (owner + "." + name)
                        work.mkdir()
                        execute, blocker = prepare(tools, work, owner, name)
                        for variant in ("Pure", "Fast"):
                            key = f"test.datetimetester.{owner}_{variant}.{name}"
                            if blocker is None:
                                executions[key] = execute
                            else:
                                preflight[(key, 1)] = blocker
                report = run_evidence(
                    load_suite(spec["module"]),
                    executions,
                    ref,
                    level,
                    preflight=preflight,
                    exceptions=upstream_skips(ref, level),
                )
                reports.append(report)
                print(f"O{level}: {report['counts']}", flush=True)
    write_reports(args.result, reports, ref)
    required = {
        f"test.datetimetester.{owner}_{variant}.{method}"
        for owner, method in [
            ("TestDateOnly", "test_delta_non_days_ignored"),
            ("TestTimeDelta", "test_basic_attributes"),
            ("TestTimeZone", "test_str"),
        ]
        for variant in ("Pure", "Fast")
    }
    for report in reports:
        observed = {case["id"] for case in report["records"]
                    if case["classification"] == "native-body-host-fixture"}
        if required - observed or report["counts"].get("failed"):
            raise SystemExit(f"O{report['opt_level']} missing required native bodies: {sorted(required - observed)}")


if __name__ == "__main__":
    main()
