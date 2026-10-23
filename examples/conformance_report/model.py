"""Typed conformance data and diagnostic classification."""

from collections import defaultdict
from collections.abc import Sequence
from dataclasses import dataclass
from enum import StrEnum, auto
from pathlib import Path
from typing import Literal


class Source(StrEnum):
    OLD = auto()
    NEW = auto()


class Classification(StrEnum):
    TRUE_POSITIVE = auto()
    FALSE_POSITIVE = auto()
    TRUE_NEGATIVE = auto()
    FALSE_NEGATIVE = auto()

    def into_title(self, *, verb: Literal["added", "removed", "changed"]) -> str:
        match self:
            case Classification.TRUE_POSITIVE:
                return f"True positives {verb}"
            case Classification.FALSE_POSITIVE:
                return f"False positives {verb}"
            case Classification.TRUE_NEGATIVE:
                return f"True negatives {verb}"
            case Classification.FALSE_NEGATIVE:
                return f"False negatives {verb}"


class Change(StrEnum):
    ADDED = auto()
    REMOVED = auto()
    CHANGED = auto()
    UNCHANGED = auto()

    def into_title(self) -> str:
        match self:
            case Change.ADDED:
                return "Optional Diagnostics Added"
            case Change.REMOVED:
                return "Optional Diagnostics Removed"
            case Change.CHANGED:
                return "Optional Diagnostics Changed"
            case Change.UNCHANGED:
                return "Optional Diagnostics Unchanged"


@dataclass(kw_only=True, slots=True)
class Position:
    line: int
    column: int


@dataclass(kw_only=True, slots=True)
class Positions:
    begin: Position
    end: Position


@dataclass(kw_only=True, slots=True)
class Location:
    path: Path
    positions: Positions


@dataclass(kw_only=True, slots=True)
class WireLocation:
    path: str
    positions: Positions


@dataclass(kw_only=True, slots=True)
class WireDiagnostic:
    check_name: str
    description: str
    severity: str
    location: WireLocation


@dataclass(kw_only=True, slots=True)
class TyDiagnostic:
    check_name: str
    description: str
    severity: str
    location: Location
    source: Source

    def __str__(self) -> str:
        return (
            f"{self.location.path}:{self.location.positions.begin.line}:"
            f"{self.location.positions.begin.column}: "
            f"{self.severity_for_display}[{self.check_name}] {self.description}"
        )

    @property
    def severity_for_display(self) -> str:
        return {
            "major": "error",
            "minor": "warning",
        }.get(self.severity, "unknown")


def diagnostics_are_equivalent(a: list[TyDiagnostic], b: list[TyDiagnostic]) -> bool:
    """Compare two diagnostic lists for equality, ignoring the ``source`` field."""

    def fingerprint(d: TyDiagnostic) -> tuple[str, str, str, str, int, int]:
        return (
            d.check_name,
            d.description,
            d.severity,
            str(d.location.path),
            d.location.positions.begin.line,
            d.location.positions.begin.column,
        )

    return sorted(map(fingerprint, a)) == sorted(map(fingerprint, b))


@dataclass(kw_only=True, slots=True)
class ExpectedError:
    description: str
    location: Location
    optional: bool
    tag: str | None
    multi: bool

    @property
    def key(self) -> str:
        """Key to group expected errors by path and beginning line or path and tag."""
        return (
            f"{self.location.path.as_posix()}:{self.location.positions.begin.line}"
            if self.tag is None
            else f"{self.location.path.as_posix()}:{self.tag}"
        )


@dataclass(kw_only=True, slots=True)
class TestCase:
    key: str
    old: list[TyDiagnostic]
    new: list[TyDiagnostic]
    expected: list[ExpectedError]

    @property
    def change(self) -> Change:
        if self.new and not self.old:
            return Change.ADDED
        elif self.old and not self.new:
            return Change.REMOVED
        elif (
            self.old and self.new and not diagnostics_are_equivalent(self.old, self.new)
        ):
            return Change.CHANGED
        else:
            return Change.UNCHANGED

    @property
    def optional(self) -> bool:
        return bool(self.expected) and all(error.optional for error in self.expected)

    @property
    def multi(self) -> bool:
        return bool(self.expected) and all(error.multi for error in self.expected)

    @property
    def path(self) -> Path:
        if self.new:
            return self.new[0].location.path
        if self.old:
            return self.old[0].location.path
        if self.expected:
            return self.expected[0].location.path
        raise ValueError(f"No diagnostics in test case {self.key}")

    def diagnostics_by_source(self, source: Source) -> list[TyDiagnostic]:
        return self.old if source == Source.OLD else self.new

    def classify(self, source: Source) -> Classification:
        diagnostics = self.diagnostics_by_source(source)

        if diagnostics:
            if self.optional:
                return Classification.TRUE_POSITIVE
            if self.expected:
                distinct_lines = len(
                    {d.location.positions.begin.line for d in diagnostics}
                )
                expected_max = len(self.expected) if self.multi else 1
                if 1 <= distinct_lines <= expected_max:
                    return Classification.TRUE_POSITIVE
                else:
                    return Classification.FALSE_POSITIVE
            else:
                return Classification.FALSE_POSITIVE

        elif self.expected:
            if self.optional:
                return Classification.TRUE_NEGATIVE
            return Classification.FALSE_NEGATIVE

        else:
            return Classification.TRUE_NEGATIVE


@dataclass(kw_only=True, slots=True)
class Statistics:
    true_positives: int = 0
    false_positives: int = 0
    false_negatives: int = 0
    total_diagnostics: int = 0

    @property
    def precision(self) -> float:
        if self.true_positives + self.false_positives > 0:
            return self.true_positives / (self.true_positives + self.false_positives)
        return 0.0

    @property
    def recall(self) -> float:
        if self.true_positives + self.false_negatives > 0:
            return self.true_positives / (self.true_positives + self.false_negatives)
        else:
            return 0.0


@dataclass(kw_only=True, slots=True)
class DiagnosticEntry:
    title: str
    test_case: TestCase
    source: Source | None


@dataclass(kw_only=True, slots=True)
class FileStats:
    path: Path
    old: Statistics
    new: Statistics

    @property
    def old_passes(self) -> bool:
        return self.old.false_positives == 0 and self.old.false_negatives == 0

    @property
    def new_passes(self) -> bool:
        return self.new.false_positives == 0 and self.new.false_negatives == 0

    @property
    def total_change(self) -> int:
        return (
            abs(self.new.true_positives - self.old.true_positives)
            + abs(self.new.false_positives - self.old.false_positives)
            + abs(self.new.false_negatives - self.old.false_negatives)
        )


def compute_stats(test_cases: list[TestCase], source: Source) -> Statistics:
    stats = Statistics()
    for tc in test_cases:
        match tc.classify(source):
            case Classification.TRUE_POSITIVE:
                stats.true_positives += 1
            case Classification.FALSE_POSITIVE:
                stats.false_positives += 1
            case Classification.FALSE_NEGATIVE:
                stats.false_negatives += 1
            case Classification.TRUE_NEGATIVE:
                pass
        stats.total_diagnostics += len(tc.diagnostics_by_source(source))
    return stats


def group_diagnostics_by_key(
    old: list[TyDiagnostic],
    new: list[TyDiagnostic],
    expected: list[ExpectedError],
) -> list[TestCase]:
    # Build a lookup from (filename, line) to tag so ty diagnostics on a tagged
    # line can be grouped with all other expected errors sharing that tag.
    tagged_lines: dict[tuple[str, int], str] = {
        (e.location.path.name, e.location.positions.begin.line): e.tag
        for e in expected
        if e.tag is not None
    }

    def ty_key(diag: TyDiagnostic) -> str:
        tag = tagged_lines.get(
            (diag.location.path.name, diag.location.positions.begin.line)
        )
        return (
            f"{diag.location.path.as_posix()}:{tag}"
            if tag is not None
            else f"{diag.location.path.as_posix()}:{diag.location.positions.begin.line}"
        )

    old_by_key: defaultdict[str, list[TyDiagnostic]] = defaultdict(list)
    new_by_key: defaultdict[str, list[TyDiagnostic]] = defaultdict(list)
    expected_by_key: defaultdict[str, list[ExpectedError]] = defaultdict(list)

    for diag in old:
        old_by_key[ty_key(diag)].append(diag)
    for diag in new:
        new_by_key[ty_key(diag)].append(diag)
    for err in expected:
        expected_by_key[err.key].append(err)

    all_keys = sorted(old_by_key.keys() | new_by_key.keys() | expected_by_key.keys())
    return [
        TestCase(
            key=key,
            old=old_by_key[key],
            new=new_by_key[key],
            expected=expected_by_key[key],
        )
        for key in all_keys
    ]


def collect_file_stats(
    test_cases: list[TestCase], test_files: Sequence[Path]
) -> list[FileStats]:
    # `test_cases` only contain files where `ty` generates a diagnostic
    # We expand this with the full set of `test_files` to ensure we don't undercount
    path_to_cases: dict[Path, list[TestCase]] = {
        path.resolve(): [] for path in test_files
    }
    for tc in test_cases:
        path_to_cases[tc.path].append(tc)
    return [
        FileStats(
            path=path,
            old=compute_stats(cases, Source.OLD),
            new=compute_stats(cases, Source.NEW),
        )
        for path, cases in path_to_cases.items()
    ]
