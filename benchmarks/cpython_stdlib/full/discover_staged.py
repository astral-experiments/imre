"""Compare CPython's discovery in the staged test tree with its complete pinned ledgers."""
import argparse
from collections import Counter
import importlib
import json
from pathlib import Path
import sys
import sysconfig
import tomllib
import unittest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from reference_report import checked_reference
sys.path.insert(0, str(HERE.parent))
from suite_inventory import cases, discovery_resources, load_suite


def suite_metadata(mod):
    manifest = HERE / mod / "UPSTREAM.json"
    return json.loads(manifest.read_bytes()) if manifest.exists() else {}


def suite_names(mod, default, refs, metadata=None):
    if metadata is None:
        metadata = suite_metadata(mod)
    names = []
    for path in refs:
        name = json.loads(path.read_bytes()).get("module")
        # An aggregate ledger can name the stdlib owner (e.g. collections)
        # while its manifest assigns several actual test package entries.
        if name is None or name == mod:
            names.extend(metadata.get("modules") or [metadata.get("module", "test." + default)])
        elif name.startswith("test."):
            names.append(name)
        else:
            raise ValueError(f"{path}: expected a test entry point or its owner {mod!r}")
    return list(dict.fromkeys(names))


def configuration():
    if not sysconfig.get_config_var("Py_GIL_DISABLED"):
        return "gil"
    if sys._is_gil_enabled():
        raise ValueError("free-threaded reference ledgers require the GIL to be disabled")
    return "free-threaded"


def free_threaded_reference(ref):
    return ref.name == f"reference-free-threading-{sys.platform}.json"


def reference_files(directory):
    free = configuration() == "free-threaded"
    return [ref for ref in sorted(directory.glob(f"reference*-{sys.platform}.json"))
            if free_threaded_reference(ref) == free]


def suite_owners():
    # Ordinary package-level GIL skips remain in their original regular ledger.
    owners = tomllib.loads((HERE / "scope.toml").read_text())["wave_one"] if configuration() == "gil" else {}
    for ref in sorted(HERE.glob(f"*/reference*-{sys.platform}.json")):
        if free_threaded_reference(ref) != (configuration() == "free-threaded"):
            continue
        owners.setdefault(ref.parent.name, "test_" + ref.parent.name)
    return owners


def early_skips(suite, reason, entries):
    """Account for an import-time skip only if the ledger has the same IDs and reason."""
    result = {}
    for name, outcome in entries:
        if name.startswith(suite + "."):
            if name in result or outcome != {"skipped": reason}:
                raise ValueError(f"{suite}: import-time skip disagrees with {name}: {outcome}")
            result[name] = outcome
    if not result:
        raise ValueError(f"unaccounted import-time skip: {suite}: {reason}")
    return result


def main():
    primary = suite_owners()
    release = json.loads((HERE.parent / "UPSTREAM.json").read_bytes())
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("staged_root", type=Path)
    parser.add_argument("modules", nargs="*", help="module owners; default: all recorded suites")
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or "v" + ".".join(map(str, sys.version_info[:3])) != release["tag"]:
        parser.error(f"use a test-enabled CPython {release['tag']}")
    modules = args.modules or list(primary)
    if unknown := set(modules) - primary.keys():
        parser.error(f"no suite assignments for {sorted(unknown)}")
    lib = args.staged_root.resolve() / "Lib"
    sys.path.insert(0, str(lib))
    if Path(importlib.import_module("test").__file__).resolve() != lib / "test/__init__.py":
        raise ValueError("the test package must come from the staged tree")
    total = skipped = 0
    for mod in modules:
        entries = []
        refs = reference_files(HERE / mod)
        for ref in refs:
            _, results = checked_reference(ref, release["commit"], sys.platform)
            entries.extend(results)
        expected = Counter(name for name, _ in entries)
        if not expected:
            raise ValueError(f"no {sys.platform} reference ledger for {mod}")
        seen = Counter()
        early = 0
        metadata = suite_metadata(mod)
        with discovery_resources(metadata):
            for suite in suite_names(mod, primary[mod], refs, metadata):
                try:
                    module = importlib.import_module(suite)
                    if not Path(module.__file__).resolve().is_relative_to(lib / "test"):
                        raise ValueError(f"{suite} resolved outside the staged tree")
                    seen.update(case.id() for case in cases(load_suite(suite)))
                except unittest.SkipTest as exc:
                    skips = early_skips(suite, str(exc), entries)
                    seen.update(skips.keys())
                    early += len(skips)
        if seen != expected:
            raise ValueError(f"{mod}: missing {dict(expected-seen)}, extra {dict(seen-expected)}")
        print(f"{mod}: {seen.total() - early} discovered, {early} skipped before discovery")
        total += seen.total()
        skipped += early
    print(f"{total - skipped} discovered; {skipped} original import-time skips; {total} occurrences accounted for")


if __name__ == "__main__":
    main()
