"""Read frozen inputs and scan annotations with Ruff's conformance pattern."""

import re
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path

from relar.json import decode

from conformance_report.model import ExpectedError, Location, Position, Positions, Source, TyDiagnostic, WireDiagnostic


CONFORMANCE_ERROR_PATTERN = re.compile(
    r"""
    \#\s*E                  # "# E" begins each error
    (?P<optional>\?)?       # Optional '?' (E?) indicates that an error is optional
    (?:                     # An optional tag for errors that may appear on multiple lines at most once
        \[
            (?P<tag>[^+\]]+)    # identifier
            (?P<multi>\+)?      # '+' indicates that an error may occur more than once on tagged lines
        \]
    )?
    (?:
        \s*:\s*(?P<description>.*) # optional description
    )?
    """,
    re.VERBOSE,
)


@dataclass(slots=True)
class PathAlias:
    raw: str
    path: str


@dataclass(slots=True)
class Manifest:
    suite_revision: str
    test_files: list[str]
    diagnostic_paths: list[PathAlias]


@dataclass(slots=True)
class SourceText:
    path: Path
    text: str


@dataclass(slots=True)
class Inputs:
    manifest: Manifest
    paths: dict[str, Path]
    sources: list[SourceText]
    old: str
    new: str


@dataclass(slots=True)
class Annotation:
    start: int
    end: int
    optional: bool
    tag: str | None
    multi: bool
    description: str


def scan_annotation(line: str) -> Annotation | None:
    if match := re.search(CONFORMANCE_ERROR_PATTERN, line):
        return Annotation(
            match.start(), match.end(), match.group("optional") is not None,
            match.group("tag"), match.group("multi") is not None,
            match.group("description") or "Missing",
        )
    return None


def collect_expected_diagnostics(test_files: Sequence[Path]) -> list[ExpectedError]:
    errors: list[ExpectedError] = []
    for file in test_files:
        for idx, line in enumerate(file.read_text().splitlines(), 1):
            if match := re.search(CONFORMANCE_ERROR_PATTERN, line):
                prefix = line[: match.start()].lstrip()
                if not prefix or prefix.startswith("#"):
                    continue

                errors.append(
                    ExpectedError(
                        description=(match.group("description") or "Missing"),
                        location=Location(
                            path=file,
                            positions=Positions(
                                begin=Position(
                                    line=idx,
                                    column=match.start(),
                                ),
                                end=Position(
                                    line=idx,
                                    column=match.end(),
                                ),
                            ),
                        ),
                        optional=match.group("optional") is not None,
                        tag=(
                            f"{file.name}:{match.group('tag')}"
                            if match.group("tag")
                            else None
                        ),
                        multi=match.group("multi") is not None,
                    )
                )

    assert errors, "Failed to discover any expected diagnostics!"
    return errors


def collect_frozen_diagnostics(sources: list[SourceText]) -> list[ExpectedError]:
    errors: list[ExpectedError] = []
    for source in sources:
        for number, line in enumerate(source.text.splitlines(), 1):
            annotation = scan_annotation(line)
            if annotation is None:
                continue
            prefix = line[:annotation.start].lstrip()
            if not prefix or prefix.startswith("#"):
                continue
            tag: str | None = None
            raw_tag = annotation.tag
            if raw_tag is not None:
                tag = f"{source.path.name}:{raw_tag}"
            errors.append(ExpectedError(
                description=annotation.description,
                location=Location(path=source.path, positions=Positions(
                    begin=Position(line=number, column=annotation.start),
                    end=Position(line=number, column=annotation.end),
                )),
                optional=annotation.optional, tag=tag, multi=annotation.multi,
            ))
    assert errors, "Failed to discover any expected diagnostics!"
    return errors


def collect_ty_diagnostics(text: str, source: Source, paths: dict[str, Path]) -> list[TyDiagnostic]:
    wire = decode(text, list[WireDiagnostic])
    result: list[TyDiagnostic] = []
    for diagnostic in wire:
        if diagnostic.severity != "major":
            continue
        canonical = paths[diagnostic.location.path]
        result.append(TyDiagnostic(
            check_name=diagnostic.check_name,
            description=diagnostic.description.replace(f"{diagnostic.check_name}: ", ""),
            severity=diagnostic.severity,
            location=Location(path=canonical, positions=diagnostic.location.positions),
            source=source,
        ))
    return result


def read_inputs(manifest_file: str, old_file: str, new_file: str) -> Inputs:
    manifest = decode(Path(manifest_file).read_text(encoding="utf-8"), Manifest)
    paths: dict[str, Path] = {}
    for alias in manifest.diagnostic_paths:
        if alias.raw in paths:
            raise ValueError("duplicate diagnostic path alias")
        paths[alias.raw] = Path(alias.path)
    seen: set[str] = set()
    sources: list[SourceText] = []
    for path in manifest.test_files:
        if path in seen:
            raise ValueError("duplicate conformance test path")
        seen.add(path)
        file = Path(path)
        sources.append(SourceText(file, file.read_text(encoding="utf-8")))
    return Inputs(
        manifest, paths, sources,
        Path(old_file).read_text(encoding="utf-8"),
        Path(new_file).read_text(encoding="utf-8"),
    )
