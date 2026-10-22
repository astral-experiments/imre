"""Observe the reporter's real classification and statistics over preloaded groups."""

from sys import argv

from conformance_report.model import Source, Statistics, TestCase, compute_stats, group_diagnostics_by_key
from conformance_report.parse import collect_expected_diagnostics, collect_ty_diagnostics, read_inputs


def cases_key(case: TestCase) -> str:
    return case.key


def load_cases(manifest: str, old_path: str, new_path: str) -> list[TestCase]:
    inputs = read_inputs(manifest, old_path, new_path)
    expected = collect_expected_diagnostics(inputs.sources)
    old = collect_ty_diagnostics(inputs.old, Source.OLD, inputs.paths)
    new = collect_ty_diagnostics(inputs.new, Source.NEW, inputs.paths)
    return sorted(group_diagnostics_by_key(old, new, expected), key=cases_key)


def score(stats: Statistics) -> int:
    return stats.true_positives + 3 * stats.false_positives + 5 * stats.false_negatives + 7 * stats.total_diagnostics


def run(cases: list[TestCase], rounds: int, dump: bool) -> None:
    if dump:
        for case in cases:
            print(f"{case.key}\t{case.classify(Source.OLD)}\t{case.classify(Source.NEW)}\t{case.change}")
    checksum = 0
    for _ in range(rounds):
        old = compute_stats(cases, Source.OLD)
        new = compute_stats(cases, Source.NEW)
        checksum += score(old) + 11 * score(new)
        if dump:
            print(f"{old.true_positives} {old.false_positives} {old.false_negatives} {old.total_diagnostics}")
            print(f"{new.true_positives} {new.false_positives} {new.false_negatives} {new.total_diagnostics}")
    print(f"{len(cases)} {checksum}")


run(load_cases(argv[1], argv[2], argv[3]), int(argv[4]), argv[5] == "dump")
