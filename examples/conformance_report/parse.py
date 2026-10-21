"""Read frozen inputs and scan the same annotation grammar as Ruff's regex."""

from dataclasses import dataclass
from pathlib import Path

from relar.json import decode

from conformance_report.model import ExpectedError, Location, Position, Positions, TyDiagnostic, WireDiagnostic


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
    path: str
    text: str


@dataclass(slots=True)
class Inputs:
    manifest: Manifest
    paths: dict[str, str]
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
    r"""Match the first ``#\s*E...`` occurrence, including regex fallback cases."""
    start = line.find("#")
    while start >= 0:
        suffix = line[start + 1:]
        trimmed = suffix.lstrip()
        position = start + 1 + len(suffix) - len(trimmed)
        if trimmed.startswith("E"):
            position += 1
            optional = line[position:position + 1] == "?"
            if optional:
                position += 1
            tag: str | None = None
            multi = False
            # The regex's whole tag group is optional. An invalid tag leaves
            # the match at E (or E?), rather than rejecting the annotation.
            if line[position:position + 1] == "[":
                close = line.find("]", position + 1)
                if close >= 0:
                    candidate = line[position + 1:close]
                    candidate_multi = candidate[-1:] == "+"
                    if candidate_multi:
                        candidate = candidate[:-1]
                    if candidate and candidate.find("+") < 0:
                        tag = candidate
                        multi = candidate_multi
                        position = close + 1
            description = "Missing"
            suffix = line[position:].lstrip()
            if suffix.startswith(":"):
                description = suffix[1:].lstrip() or "Missing"
                position = len(line)
            return Annotation(start, position, optional, tag, multi, description)
        start = line.find("#", start + 1)
    return None


def collect_expected_diagnostics(sources: list[SourceText]) -> list[ExpectedError]:
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
                tag = f"{Path(source.path).name}:{raw_tag}"
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


def collect_ty_diagnostics(text: str, source: str, paths: dict[str, str]) -> list[TyDiagnostic]:
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
    paths: dict[str, str] = {}
    for alias in manifest.diagnostic_paths:
        if alias.raw in paths:
            raise ValueError("duplicate diagnostic path alias")
        paths[alias.raw] = alias.path
    seen: set[str] = set()
    sources: list[SourceText] = []
    for path in manifest.test_files:
        if path in seen:
            raise ValueError("duplicate conformance test path")
        seen.add(path)
        sources.append(SourceText(path, Path(path).read_text(encoding="utf-8")))
    return Inputs(
        manifest, paths, sources,
        Path(old_file).read_text(encoding="utf-8"),
        Path(new_file).read_text(encoding="utf-8"),
    )
