"""Report native argparse execution for every pinned CPython unittest occurrence."""

import argparse
from concurrent.futures import ProcessPoolExecutor
import json
from pathlib import Path
import re
import shutil
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from native_cases import HERE, measure as parse_routines
from native_methods import measure as source_methods
from help_cases import measure as help_routines
from direct_cases import measure as direct_routines
from registration_cases import measure as registration_routines
from subparser_cases import measure as subparser_routines
from mutex_cases import measure as mutex_routines
from suite_evidence import build_report, occurrence_keys, write_reports
from suite_inventory import cases, load_suite, verify_files
from suite_native import Toolchain


def measure(toolchain, manifest, reference, source, revision):
    module = manifest["module"]
    inventory = [(c.id(), f"{type(c).__module__}.{type(c).__qualname__}") for c in cases(load_suite(module))]
    keys = list(occurrence_keys(name for name, _ in inventory))
    # The pinned module has unique IDs. Do not silently conflate future repeats.
    if len(set(name for name, _ in keys)) != len(keys):
        raise ValueError("argparse adapters require an explicit fixture variant for repeated IDs")
    per_adapter = [{}, {}, {}]
    source_methods(toolchain.level, toolchain, manifest, reference["reference"], source, per_adapter[0])
    direct_routines(toolchain, module, reference["reference"], source, per_adapter[0])
    registration_routines(toolchain, module, reference["reference"], source, per_adapter[0])
    subparser_routines(toolchain, module, reference["reference"], per_adapter[0])
    mutex_routines(toolchain, module, reference["reference"], per_adapter[0])
    help_routines(toolchain, module, reference["reference"], per_adapter[1])
    parse_routines(toolchain, module, reference["reference"], per_adapter[2])
    observations = {}
    for result in per_adapter:
        if observations.keys() & result.keys():
            raise ValueError("two adapters reported the same argparse occurrence")
        observations.update(result)
    if observations.keys() != {name for name, _ in keys}:
        raise ValueError("no adapter attempted one or more discovered argparse routines")
    report = build_report(reference, toolchain.level, inventory, {key: observations[key[0]] for key in keys})
    report["imre_revision"] = revision
    print(f"-O{toolchain.level}: {report['counts']}", flush=True)
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--imre", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--imre-revision", required=True, help="full Git revision of both native artifacts")
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--result", required=True, type=Path)
    opts = parser.parse_args()
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    if sys.implementation.name != "cpython" or "v" + ".".join(map(str, sys.version_info[:3])) != manifest["tag"]:
        parser.error(f"run with test-enabled CPython {manifest['tag']}")
    clang = shutil.which(opts.clang)
    if not clang or not re.fullmatch(r"[0-9a-f]{40}", opts.imre_revision):
        parser.error("expected a clang executable and full Imre commit")
    import test
    from test import test_argparse as upstream
    verify_files(HERE / "upstream", manifest["files"])
    verify_files(Path(test.__file__).resolve().parents[2], {
        **manifest["reference_files"], **{k: v for k, v in manifest["files"].items() if k.startswith("Lib/test/")}})
    reference = json.loads((HERE / f"reference-{sys.platform}.json").read_text())
    if reference["cpython_commit"] != manifest["commit"] or reference["platform"] != sys.platform:
        raise ValueError("reference commit or platform differs from the current run")
    with ProcessPoolExecutor(max_workers=2) as pool:
        jobs = [pool.submit(measure, Toolchain(opts.imre.resolve(), opts.runtime.resolve(), Path(clang), lv),
                            manifest, reference, Path(upstream.__file__), opts.imre_revision) for lv in (0, 3)]
        reports = [job.result() for job in jobs]
    write_reports(opts.result, reports, reference)
    return int(any(k in r["counts"] for r in reports for k in ("failed", "unexpected-skip", "reference-disagreement")))


if __name__ == "__main__":
    sys.exit(main())
