"""Rerun complete CPython reference suites in the disposable test prefix."""
import argparse
from collections import Counter, defaultdict
import importlib
import json
import os
from pathlib import Path
import subprocess
import sys
import sysconfig
import tempfile
import unittest
from xml.etree.ElementTree import ParseError

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from discover_staged import configuration, early_skips, reference_files, suite_names, suite_owners
from reference_report import checked_reference
from stage_tests import verify_prefix
sys.path.insert(0, str(HERE.parent))
from suite_inventory import load_suite, passing_subtest_skips, reference_results

# These are the resource configurations used by the checked-in module ledgers.
RESOURCES = {"collections": "cpu", "json": "cpu", "logging": "all", "pathlib": "all",
             "socketserver": "network"}
INSTALLED_ONLY = {f"test.test_subprocess.{cls}.test_executable_without_cwd"
                  for cls in ("ProcessTestCase", "ProcessTestCaseNoPoll")}


def run_group(mod, primary, pin, out):
    refs = reference_files(HERE / mod)
    expected = defaultdict(list)
    recorded_subtests, subtest_aliases, subtest_differences = {}, {}, {}
    for ref in refs:
        raw, entries = checked_reference(ref, pin, sys.platform)
        if "subtest_skips" in raw:
            suite = raw["module"]
            _, observed, aliases = passing_subtest_skips(load_suite(suite))
            recorded_subtests[suite] = observed
            subtest_aliases.update(aliases)
            if observed != raw["subtest_skips"]:
                subtest_differences[suite] = {"expected": raw["subtest_skips"], "observed": observed}
        for key, outcome in entries:
            expected[key].append(outcome)
    if not expected:
        raise ValueError(f"missing reference ledger for {mod}")
    suites = []
    early = {}
    for suite in suite_names(mod, primary, refs):
        try:
            importlib.import_module(suite)
        except unittest.SkipTest as exc:
            early.update(early_skips(suite, str(exc), ((k, v) for k, values in expected.items() for v in values)))
        else:
            suites.append(suite.removeprefix("test."))
    xml = out / f"{mod}.xml"
    # The upstream importlib suite checks how loaders create and validate .pyc files.
    options = [] if mod == "importlib" else ["-B"]
    if mod == "pathlib":
        options += ["-X", "warn_default_encoding"]
    resources = ["-u", RESOURCES[mod]] if mod in RESOURCES else []
    with tempfile.TemporaryDirectory() as work, (out / f"{mod}.log").open("w") as log:
        command = [sys.executable, *options, "-m", "test", "--single-process",
                   "--tempdir", work, "--randseed", "1", *resources, "--junit-xml", str(xml), *suites]
        # Popen's original executable tests deliberately pass a fictitious argv[0].
        # Tell their relocated Python where its standard library is, even then.
        env = {k: v for k, v in os.environ.items() if k != "PYTHONPATH"}
        proc = subprocess.run(command, cwd=work, stdout=log, stderr=subprocess.STDOUT,
                              env={**env, "PYTHONHOME": sys.prefix})
    result = compare_group(xml, expected, early, proc.returncode, subtest_aliases)
    if recorded_subtests:
        result["subtest_skips"] = recorded_subtests
        result["differences"].update(subtest_differences)
    return result


def compare_group(xml, expected, early, returncode, subtests=None):
    """Only a complete set of identified test occurrences can supply a module's counts."""
    try:
        actual = reference_results(xml, Counter({k: len(v) for k, v in expected.items() if k not in early}), subtests) | early
    except (FileNotFoundError, ParseError, ValueError) as exc:
        return {"exit": returncode, "expected_instances": sum(map(len, expected.values())),
                "instances": None, "outcomes": {}, "installed_only_passes": [], "differences": {},
                "incomplete": f"{xml.name}: {exc}; see {xml.with_suffix('.log').name}"}
    counts = Counter()
    differences = {}
    installed_passes = []
    for name, outcomes in actual.items():
        outcomes = outcomes if isinstance(outcomes, list) else [outcomes]
        counts.update(v if isinstance(v, str) else next(iter(v)) for v in outcomes)
        if expected[name] == outcomes:
            continue
        if (name in INSTALLED_ONLY and not sysconfig.is_python_build()
                and expected[name] == [{"skipped": "need an installed Python. See #7774"}]
                and outcomes == ["pass"]):
            installed_passes.append(name)
        else:
            differences[name] = {"expected": expected[name], "observed": outcomes}
    return {"exit": returncode, "instances": counts.total(), "outcomes": dict(counts),
            "installed_only_passes": sorted(installed_passes), "differences": differences}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True, help="empty result directory for logs and JUnit XML")
    parser.add_argument("modules", nargs="*", help="default: all modules with reference ledgers")
    args = parser.parse_args()
    release = json.loads((HERE.parent / "UPSTREAM.json").read_bytes())
    if sys.implementation.name != "cpython" or "v" + ".".join(map(str, sys.version_info[:3])) != release["tag"]:
        parser.error(f"use the disposable {release['tag']} reference prefix")
    actual_test = Path(importlib.import_module("test").__file__).resolve()
    stdlib = Path(sysconfig.get_path("stdlib")).resolve()
    if actual_test != stdlib / "test/__init__.py":
        parser.error("the test package must belong to the disposable interpreter prefix")
    count = verify_prefix(stdlib)
    wave = suite_owners()
    modules = args.modules or list(wave)
    if bad := set(modules) - wave.keys():
        parser.error(f"unknown module: {sorted(bad)}")
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    if any(out.iterdir()):
        parser.error("use an empty output directory")
    results = {}
    for mod in modules:
        result = results[mod] = run_group(mod, wave[mod], release["commit"], out)
        if "incomplete" in result:
            print(f"{mod}: incomplete reference: {result['incomplete']}", flush=True)
        else:
            print(f"{mod}: {result['outcomes']}; {len(result['differences'])} unexpected differences", flush=True)
    (out / "summary.json").write_text(json.dumps({"cpython_commit": release["commit"],
        "platform": sys.platform, "interpreter": "CPython", "configuration": configuration(),
        "verified_fixture_files": count,
        "modules": results}, indent=2) + "\n")
    return int(any(r["exit"] or r["differences"] or "incomplete" in r for r in results.values()))


if __name__ == "__main__":
    raise SystemExit(main())
