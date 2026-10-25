"""Discover a typing checkout, run both analyzers, and publish one report."""

import os
import subprocess
import tempfile
from dataclasses import dataclass
from pathlib import Path

from imre.json import decode
from imre.toml import decode as decode_toml

from conformance_report.app import render_diagnostics
from conformance_report.model import Source, WireDiagnostic
from conformance_report.parse import collect_expected_diagnostics, collect_ty_diagnostics


@dataclass(slots=True)
class TestGroup:
    name: str
    href: str


@dataclass(slots=True)
class LiveOptions:
    suite: str
    old_ty: str
    new_ty: str
    output: str
    suite_revision: str
    format: str
    force_summary_table: bool
    timeout: float


def parse_live_options(arguments: list[str]) -> LiveOptions:
    suite = ""
    old_ty = ""
    new_ty = ""
    output = ""
    revision = "835b70f3f795f8e061b1f7621178515b0a77a0ad"
    format = "github"
    force_summary_table = False
    timeout = 15.0
    index = 1
    while index < len(arguments):
        flag = arguments[index]
        if flag == "--force-summary-table":
            force_summary_table = True
            index += 1
            continue
        if index + 1 >= len(arguments):
            raise ValueError("expected a value after " + flag)
        value = arguments[index + 1]
        if flag == "--suite":
            suite = value
        elif flag == "--old-ty":
            old_ty = value
        elif flag == "--new-ty":
            new_ty = value
        elif flag == "--output":
            output = value
        elif flag == "--suite-revision":
            revision = value
        elif flag == "--format":
            format = value
        elif flag == "--timeout":
            timeout = float(value)
        else:
            raise ValueError("unknown argument: " + flag)
        index += 2
    if not suite or not old_ty or not new_ty or not output:
        raise ValueError("expected --suite CHECKOUT/conformance --old-ty BIN --new-ty BIN --output REPORT")
    if format != "github" and format != "diff":
        raise ValueError("format must be github or diff")
    if timeout <= 0.0 or timeout != timeout or timeout == float("inf"):
        raise ValueError("timeout must be finite and positive")
    return LiveOptions(suite, old_ty, new_ty, output, revision, format, force_summary_table, timeout)


def discover_test_files(suite: Path) -> list[Path]:
    groups = decode_toml((suite / "src" / "test_groups.toml").read_text(encoding="utf-8"), dict[str, TestGroup])
    tests = suite / "tests"
    test_files: list[Path] = []
    # Upstream discovers Python sources before stubs and excludes support files.
    candidates = list(tests.glob("*.py"))
    candidates.extend(list(tests.glob("*.pyi")))
    for path in candidates:
        if path.name.split("_")[0] in groups:
            test_files.append(path.resolve(strict=True))
    if not test_files:
        raise ValueError("no conformance test files discovered")
    return test_files


def analyzer_arguments(suite: Path, test_files: list[Path]) -> list[str]:
    return [
        "check", "--python-version=3.12", "--output-format=gitlab",
        "--ignore=assert-type-unspellable-subtype",
        "--error=ambiguous-protocol-member", "--error=invalid-enum-member-annotation",
        "--error=invalid-legacy-positional-parameter", "--error=mismatched-type-name",
        "--error=invalid-named-tuple-override", "--error=deprecated",
        "--error=redundant-final-classvar", "--exit-zero",
        "--extra-search-path=" + str(suite / "tests"),
        *[str(path) for path in test_files],
    ]


def run_analyzer(binary: Path, arguments: list[str], suite: Path, environment: dict[str, str], timeout: float) -> str:
    result = subprocess.run(
        [str(binary), *arguments], cwd=suite, env=environment,
        capture_output=True, text=True, encoding="utf-8", check=True, timeout=timeout,
    )
    return result.stdout


def resolve_diagnostic_paths(text: str, suite: Path, paths: dict[str, Path]) -> None:
    for diagnostic in decode(text, list[WireDiagnostic]):
        raw = diagnostic.location.path
        path = Path(raw)
        if not path.is_absolute():
            path = suite / path
        paths[raw] = path.resolve()


def live_main(arguments: list[str]) -> None:
    options = parse_live_options(arguments)
    suite = Path(options.suite).resolve(strict=True)
    old_ty = Path(options.old_ty).resolve(strict=True)
    new_ty = Path(options.new_ty).resolve(strict=True)
    output = Path(options.output)
    with tempfile.TemporaryDirectory(dir=output.parent, prefix=".imre-conformance-") as name:
        workspace = Path(name)
        test_files = discover_test_files(suite)
        expected = collect_expected_diagnostics(test_files)
        command = analyzer_arguments(suite, test_files)
        environment = os.environ.copy()
        paths: dict[str, Path] = {}
        old = run_analyzer(old_ty, command, suite, environment, options.timeout)
        resolve_diagnostic_paths(old, suite, paths)
        new = run_analyzer(new_ty, command, suite, environment, options.timeout)
        resolve_diagnostic_paths(new, suite, paths)
        rendered = render_diagnostics(
            expected, collect_ty_diagnostics(old, Source.OLD, paths),
            collect_ty_diagnostics(new, Source.NEW, paths), test_files,
            options.suite_revision, options.format, options.force_summary_table,
        )
        staged = workspace / "report.md"
        staged.write_text(rendered, encoding="utf-8")
        staged.replace(output)
