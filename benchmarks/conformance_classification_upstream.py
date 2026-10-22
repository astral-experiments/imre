"""Drive the immutable Ruff implementation using its real TestCase and Statistics records."""

from collections import Counter
from contextlib import redirect_stdout
from io import StringIO
from pathlib import Path
from types import SimpleNamespace
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "examples"))
from conformance_report import original


def load_cases(inputs, upstream):
    expected = upstream.collect_expected_diagnostics(inputs.sources)
    for error in expected:
        error.location.path = error.location.path.path
    old = original.diagnostics(upstream, inputs.old, upstream.Source.OLD, inputs.paths)
    new = original.diagnostics(upstream, inputs.new, upstream.Source.NEW, inputs.paths)
    return sorted(upstream.group_diagnostics_by_key(old=old, new=new, expected=expected), key=lambda case: case.key)


def score(stats):
    return stats.true_positives + 3 * stats.false_positives + 5 * stats.false_negatives + 7 * stats.total_diagnostics


def observe(upstream, cases, rounds, dump):
    if dump:
        for case in cases:
            print(f"{case.key}\t{case.classify(upstream.Source.OLD)}\t{case.classify(upstream.Source.NEW)}\t{case.change}")
    checksum = 0
    for _ in range(rounds):
        old = upstream.compute_stats(cases, upstream.Source.OLD)
        new = upstream.compute_stats(cases, upstream.Source.NEW)
        checksum += score(old) + 11 * score(new)
        if dump:
            print(f"{old.true_positives} {old.false_positives} {old.false_negatives} {old.total_diagnostics}")
            print(f"{new.true_positives} {new.false_positives} {new.false_negatives} {new.total_diagnostics}")
    print(f"{len(cases)} {checksum}")


def snapshot(validation, workload, source_contents=None):
    _, paths, sources, old, new = validation["_inputs"](workload, source_contents)
    upstream = validation["upstream"]()
    cases = load_cases(SimpleNamespace(paths=paths, sources=sources, old=old, new=new), upstream)
    buffer = StringIO()
    with redirect_stdout(buffer):
        observe(upstream, cases, 1, True)
    distribution = {
        "old": dict(Counter(str(case.classify(upstream.Source.OLD)) for case in cases)),
        "new": dict(Counter(str(case.classify(upstream.Source.NEW)) for case in cases)),
        "change": dict(Counter(str(case.change) for case in cases)),
        "optional": sum(case.optional for case in cases), "multi": sum(case.multi for case in cases),
    }
    assert set(distribution["old"]) | set(distribution["new"]) == {entry.value for entry in upstream.Classification}
    assert set(distribution["change"]) == {entry.value for entry in upstream.Change}
    return buffer.getvalue().encode(), distribution


if __name__ == "__main__":
    up = original.load_upstream()
    observe(up, load_cases(original.read_inputs(*sys.argv[1:4]), up), int(sys.argv[4]), sys.argv[5] == "dump")
