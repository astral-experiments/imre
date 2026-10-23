"""Run the native Codon port with Ruff's default stdout publication."""

import os
from pathlib import Path
from sys import argv

from conformance_report.app import render_diagnostics
from conformance_report.live import (
    TestGroup,
    analyzer_arguments,
    decode_toml,
    parse_live_options,
    run_analyzer,
)
from conformance_report.model import Location, Source, TyDiagnostic, WireDiagnostic
from conformance_report.parse import (
    collect_expected_diagnostics,
    decode,
)


def discover(suite: Path) -> list[Path]:
    groups = decode_toml(
        (suite / "src/test_groups.toml").read_text(encoding="utf-8"),
        dict[str, TestGroup],
    )
    tests = suite / "tests"
    # Codon's Path.glob uses a LIFO traversal. Preserve CPython's scandir order.
    return [
        p
        for suffix in (".py", ".pyi")
        for p in tests.iterdir()
        if p.name.endswith(suffix) and p.name.split("_")[0] in groups
    ]


def diagnostics(text: str, source: Source) -> list[TyDiagnostic]:
    return [
        TyDiagnostic(
            check_name=d.check_name,
            description=d.description.replace(f"{d.check_name}: ", ""),
            severity=d.severity,
            location=Location(
                path=Path(d.location.path).resolve(), positions=d.location.positions
            ),
            source=source,
        )
        for d in decode(text, list[WireDiagnostic])
        if d.severity == "major"
    ]


def main() -> None:
    options = parse_live_options(
        [a for a in argv if a != "--stdout"] + ["--output", "-"]
    )
    suite = Path(options.suite).resolve(strict=True)
    files = discover(suite)
    expected = collect_expected_diagnostics(files)
    arguments = analyzer_arguments(suite, files)
    environment = os.environ.copy()
    old = diagnostics(
        run_analyzer(
            Path(options.old_ty), arguments, suite, environment, options.timeout
        ),
        Source.OLD,
    )
    new = diagnostics(
        run_analyzer(
            Path(options.new_ty), arguments, suite, environment, options.timeout
        ),
        Source.NEW,
    )
    print(
        render_diagnostics(
            expected,
            old,
            new,
            files,
            options.suite_revision,
            options.format,
            options.force_summary_table,
        )
    )


main()
