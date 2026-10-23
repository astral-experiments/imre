"""Run the unchanged upstream analysis and the shared, fully observable workload."""

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path[:0] = [str(ROOT / "examples"), str(ROOT / "stdlib"), str(ROOT / "benchmarks")]
import conformance_grouping as observer
from conformance_report import original


def main():
    mode, manifest, old_path, new_path, rounds = sys.argv[1:]
    upstream = original.load_upstream()
    inputs = original.read_inputs(manifest, old_path, new_path)
    expected = upstream.collect_expected_diagnostics(inputs.sources)
    for error in expected:
        error.location.path = error.location.path.path
    old = original.diagnostics(upstream, inputs.old, upstream.Source.OLD, inputs.paths)
    new = original.diagnostics(upstream, inputs.new, upstream.Source.NEW, inputs.paths)
    observer.group_diagnostics_by_key = upstream.group_diagnostics_by_key
    observer.diagnostics_are_equivalent = upstream.diagnostics_are_equivalent
    observer.collect_file_stats = upstream.collect_file_stats
    observer.observe(
        mode,
        old,
        new,
        expected,
        [source.path for source in inputs.sources],
        int(rounds),
    )


if __name__ == "__main__":
    main()
