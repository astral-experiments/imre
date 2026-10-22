"""Typed conformance data and diagnostic classification."""

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
    path: str
    positions: Positions


@dataclass(kw_only=True, slots=True)
class WireDiagnostic:
    check_name: str
    description: str
    severity: str
    location: Location


@dataclass(kw_only=True, slots=True)
class TyDiagnostic:
    check_name: str
    description: str
    severity: str
    location: Location
    source: Source

    @property
    def severity_for_display(self) -> str:
        if self.severity == "major":
            return "error"
        if self.severity == "minor":
            return "warning"
        return "unknown"


def diagnostic_text(diagnostic: TyDiagnostic) -> str:
    return (
        f"{diagnostic.location.path}:{diagnostic.location.positions.begin.line}:"
        f"{diagnostic.location.positions.begin.column}: "
        f"{diagnostic.severity_for_display}[{diagnostic.check_name}] {diagnostic.description}"
    )


def fingerprint(diagnostic: TyDiagnostic) -> tuple[str, str, str, str, int, int]:
    return (
        diagnostic.check_name,
        diagnostic.description,
        diagnostic.severity,
        diagnostic.location.path,
        diagnostic.location.positions.begin.line,
        diagnostic.location.positions.begin.column,
    )


def diagnostics_are_equivalent(a: list[TyDiagnostic], b: list[TyDiagnostic]) -> bool:
    if len(a) != len(b):
        return False
    left = sorted(map(fingerprint, a))
    right = sorted(map(fingerprint, b))
    for old, new in zip(left, right):
        if old != new:
            return False
    return True


@dataclass(kw_only=True, slots=True)
class ExpectedError:
    description: str
    location: Location
    optional: bool
    tag: str | None
    multi: bool

    @property
    def key(self) -> str:
        tag = self.tag
        if tag is None:
            return f"{self.location.path}:{self.location.positions.begin.line}"
        return f"{self.location.path}:{tag}"


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
    def path(self) -> str:
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
        return 0.0


@dataclass(kw_only=True, slots=True)
class DiagnosticEntry:
    title: str
    test_case: TestCase
    source: Source | None


@dataclass(kw_only=True, slots=True)
class FileStats:
    path: str
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


def diagnostic_key(diagnostic: TyDiagnostic, tags: dict[tuple[str, int], str]) -> str:
    tag = tags.get((Path(diagnostic.location.path).name, diagnostic.location.positions.begin.line))
    if tag is None:
        return f"{diagnostic.location.path}:{diagnostic.location.positions.begin.line}"
    return f"{diagnostic.location.path}:{tag}"


def group_diagnostics_by_key(
    old: list[TyDiagnostic], new: list[TyDiagnostic], expected: list[ExpectedError]
) -> list[TestCase]:
    tagged_lines: dict[tuple[str, int], str] = {}
    for error in expected:
        tag = error.tag
        if tag is not None:
            tagged_lines[(Path(error.location.path).name, error.location.positions.begin.line)] = tag

    old_by_key: dict[str, list[TyDiagnostic]] = {}
    new_by_key: dict[str, list[TyDiagnostic]] = {}
    expected_by_key: dict[str, list[ExpectedError]] = {}
    keys: set[str] = set()
    for diagnostic in old:
        key = diagnostic_key(diagnostic, tagged_lines)
        if key not in old_by_key:
            old_by_key[key] = []
        old_by_key[key].append(diagnostic)
        keys.add(key)
    for diagnostic in new:
        key = diagnostic_key(diagnostic, tagged_lines)
        if key not in new_by_key:
            new_by_key[key] = []
        new_by_key[key].append(diagnostic)
        keys.add(key)
    for error in expected:
        key = error.key
        if key not in expected_by_key:
            expected_by_key[key] = []
        expected_by_key[key].append(error)
        keys.add(key)
    cases: list[TestCase] = []
    for key in sorted(keys):
        cases.append(TestCase(
            key=key, old=old_by_key.get(key, []), new=new_by_key.get(key, []),
            expected=expected_by_key.get(key, []),
        ))
    return cases


def collect_file_stats(test_cases: list[TestCase], test_files: list[str]) -> list[FileStats]:
    path_to_cases: dict[str, list[TestCase]] = {path: [] for path in test_files}
    for case in test_cases:
        path_to_cases[case.path].append(case)
    result: list[FileStats] = []
    for path, cases in path_to_cases.items():
        result.append(FileStats(path=path, old=compute_stats(cases, Source.OLD), new=compute_stats(cases, Source.NEW)))
    return result
