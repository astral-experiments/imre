"""Measure the conformance analysis on preloaded records and expose the full result."""

from pathlib import Path
from sys import argv

from conformance_report.model import (
    ExpectedError,
    FileStats,
    Source,
    Statistics,
    TestCase,
    TyDiagnostic,
    collect_file_stats,
    diagnostics_are_equivalent,
    group_diagnostics_by_key,
)
from conformance_report.parse import (
    collect_expected_diagnostics,
    collect_ty_diagnostics,
    read_inputs,
)


def escaped(text: str) -> str:
    return (
        text.replace("\\", "\\\\")
        .replace("\t", "\\t")
        .replace("\n", "\\n")
        .replace("\r", "\\r")
    )


def dump_diagnostics(items: list[TyDiagnostic]) -> None:
    for item in items:
        loc = item.location
        begin = loc.positions.begin
        end = loc.positions.end
        print(
            f"D\t{item.source}\t{escaped(item.check_name)}\t{escaped(item.description)}\t{escaped(item.severity)}\t{escaped(str(loc.path))}\t{begin.line}\t{begin.column}\t{end.line}\t{end.column}"
        )


def stats_text(s: Statistics) -> str:
    return f"{s.true_positives}\t{s.false_positives}\t{s.false_negatives}\t{s.total_diagnostics}"


def dump(cases: list[TestCase], files: list[FileStats]) -> None:
    for case in cases:
        equal = diagnostics_are_equivalent(case.old, case.new)
        print(f"G\t{escaped(case.key)}\t{int(equal)}")
        dump_diagnostics(case.old)
        dump_diagnostics(case.new)
        for err in case.expected:
            tag = err.tag
            tag_text = "-" if tag is None else "+" + escaped(tag)
            begin = err.location.positions.begin
            end = err.location.positions.end
            print(
                f"E\t{escaped(err.description)}\t{escaped(str(err.location.path))}\t{begin.line}\t{begin.column}\t{end.line}\t{end.column}\t{int(err.optional)}\t{tag_text}\t{int(err.multi)}"
            )
    for f in files:
        print(f"F\t{escaped(str(f.path))}\t{stats_text(f.old)}\t{stats_text(f.new)}")


def stats_score(s: Statistics) -> int:
    return (
        s.true_positives
        + 3 * s.false_positives
        + 5 * s.false_negatives
        + 7 * s.total_diagnostics
    )


def observe_aliases(cases: list[TestCase], old: list[TyDiagnostic]) -> None:
    matches = 0
    if old:
        previous = old[0].description
        marker = previous + "\x00grouping-alias-probe"
        old[0].description = marker
        for case in cases:
            for d in case.old:
                if d.description == marker:
                    matches += 1
        old[0].description = previous
    empty = sum(1 for case in cases if not case.old)
    for case in cases:
        if not case.old and old:
            case.old.append(old[0])
            break
    remaining = sum(1 for case in cases if not case.old)
    print(f"{matches} {empty} {remaining}")


def observe(
    mode: str,
    old: list[TyDiagnostic],
    new: list[TyDiagnostic],
    expected: list[ExpectedError],
    paths: list[Path],
    rounds: int,
) -> None:
    checksum = 0
    if mode == "group":
        for _ in range(rounds):
            if len(old) > 1:
                old[0], old[-1] = old[-1], old[0]
            cases = group_diagnostics_by_key(old=old, new=new, expected=expected)
            for index, case in enumerate(cases, 1):
                checksum += index * (
                    1 + len(case.old) + 3 * len(case.new) + 5 * len(case.expected)
                )
    else:
        cases = group_diagnostics_by_key(old=old, new=new, expected=expected)
        if mode == "dump":
            dump(cases, collect_file_stats(cases, paths))
            return
        if mode == "aliases":
            observe_aliases(cases, old)
            return
        for _ in range(rounds):
            if len(cases) > 1:
                cases[0], cases[-1] = cases[-1], cases[0]
            if mode == "equivalent":
                for index, case in enumerate(cases, 1):
                    if diagnostics_are_equivalent(case.old, case.new):
                        checksum += index
            elif mode == "files":
                for index, file_stats in enumerate(collect_file_stats(cases, paths), 1):
                    checksum += index * (
                        1
                        + stats_score(file_stats.old)
                        + 11 * stats_score(file_stats.new)
                    )
            else:
                raise ValueError("unknown analysis mode")
    print(checksum)


def main() -> None:
    inputs = read_inputs(argv[2], argv[3], argv[4])
    old = collect_ty_diagnostics(inputs.old, Source.OLD, inputs.paths)
    new = collect_ty_diagnostics(inputs.new, Source.NEW, inputs.paths)
    expected = collect_expected_diagnostics(inputs.sources)
    paths = [Path(path) for path in inputs.manifest.test_files]
    observe(argv[1], old, new, expected, paths, int(argv[5]))


if __name__ == "__main__":
    main()
