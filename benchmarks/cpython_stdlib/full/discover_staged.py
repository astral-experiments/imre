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


def suite_names(mod, default, refs):
    manifest = HERE / mod / "UPSTREAM.json"
    metadata = json.loads(manifest.read_bytes()) if manifest.exists() else {}
    return metadata.get("modules") or [n for p in refs if (n := json.loads(p.read_bytes()).get("module"))] or ["test." + default]


def early_skip(suite, reason, mod):
    key = suite + ".TestDeque.test_copy_race"
    if suite == "test.test_free_threading.test_collections" and reason == "GIL enabled":
        ref = json.loads((HERE / mod / f"reference-{sys.platform}.json").read_bytes())
        if ref["reference"].get(key) == {"skipped": reason}:
            return key
    raise ValueError(f"unaccounted import-time skip: {suite}: {reason}")


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
        refs = sorted((HERE / mod).glob(f"reference*-{sys.platform}.json"))
        for ref in refs:
            results = read_results(ref, release["commit"], sys.platform)
            expected.update(name for name, _ in results)
        if not expected:
            raise ValueError(f"no {sys.platform} reference ledger for {mod}")
        seen = Counter()
        early = 0
        for suite in suite_names(mod, primary[mod], refs):
            try:
                module = importlib.import_module(suite)
                if not Path(module.__file__).resolve().is_relative_to(lib / "test"):
                    raise ValueError(f"{suite} resolved outside the staged tree")
                seen.update(case.id() for case in cases(load_suite(suite)))
            except unittest.SkipTest as exc:
                # On a GIL build this package skips before discovery of its sole method.
                key = early_skip(suite, str(exc), mod)
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
