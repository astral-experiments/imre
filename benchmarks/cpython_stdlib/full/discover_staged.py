"""Compare CPython's discovery in the staged test tree with its complete pinned ledgers."""
import argparse
from collections import Counter
import importlib
import json
from pathlib import Path
import sys
import tomllib
import unittest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from reference_report import read_results
sys.path.insert(0, str(HERE.parent))
from suite_inventory import cases, load_suite


def main():
    primary = tomllib.loads((HERE / "scope.toml").read_text())["wave_one"]
    release = json.loads((HERE.parent / "UPSTREAM.json").read_bytes())
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("staged_root", type=Path)
    parser.add_argument("modules", nargs="*", help="module owners; default: all wave-one suites")
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or "v" + ".".join(map(str, sys.version_info[:3])) != release["tag"]:
        parser.error(f"use a test-enabled CPython {release['tag']}")
    modules = args.modules or list(primary)
    if unknown := set(modules) - primary.keys():
        parser.error(f"no wave-one assignments for {sorted(unknown)}")
    lib = args.staged_root.resolve() / "Lib"
    sys.path.insert(0, str(lib))
    if Path(importlib.import_module("test").__file__).resolve() != lib / "test/__init__.py":
        raise ValueError("the test package must come from the staged tree")
    total = skipped = 0
    for mod in modules:
        expected = Counter()
        names = []
        for ref in sorted((HERE / mod).glob(f"reference*-{sys.platform}.json")):
            results = read_results(ref, release["commit"], sys.platform)
            expected.update(name for name, _ in results)
            if name := json.loads(ref.read_bytes()).get("module"):
                names.append(name)
        if not expected:
            raise ValueError(f"no {sys.platform} reference ledger for {mod}")
        pin = HERE / mod / "UPSTREAM.json"
        metadata = json.loads(pin.read_bytes()) if pin.exists() else {}
        suites = metadata.get("modules") or names or ["test." + primary[mod]]
        seen = Counter()
        early = 0
        for suite in suites:
            try:
                module = importlib.import_module(suite)
                if not Path(module.__file__).resolve().is_relative_to(lib / "test"):
                    raise ValueError(f"{suite} resolved outside the staged tree")
                seen.update(case.id() for case in cases(load_suite(suite)))
            except unittest.SkipTest as exc:
                # On a GIL build this package skips before discovery of its sole method.
                if suite != "test.test_free_threading.test_collections" or str(exc) != "GIL enabled":
                    raise
                key = suite + ".TestDeque.test_copy_race"
                reference = json.loads((HERE / mod / f"reference-{sys.platform}.json").read_bytes())
                if reference["reference"].get(key) != {"skipped": str(exc)}:
                    raise
                seen[key] += 1
                early += 1
        if seen != expected:
            raise ValueError(f"{mod}: missing {dict(expected-seen)}, extra {dict(seen-expected)}")
        print(f"{mod}: {seen.total() - early} discovered, {early} skipped before discovery")
        total += seen.total()
        skipped += early
    print(f"{total - skipped} discovered; {skipped} original import-time skips; {total} occurrences accounted for")


if __name__ == "__main__":
    main()
