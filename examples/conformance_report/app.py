"""Offline CLI and reusable full-report entry point."""

from dataclasses import dataclass
from pathlib import Path

from conformance_report.model import collect_file_stats, group_diagnostics_by_key
from conformance_report.parse import Inputs, collect_expected_diagnostics, collect_ty_diagnostics, read_inputs
from conformance_report.render import render_file_stats_table, render_summary, render_test_cases


@dataclass(slots=True)
class Options:
    manifest: str
    old: str
    new: str
    format: str
    force_summary_table: bool
    output: str | None
    rounds: int


def parse_options(arguments: list[str], *, compute: bool = False) -> Options:
    manifest = ""
    old = ""
    new = ""
    format = "github"
    force_summary_table = False
    output: str | None = None
    rounds = 1
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
        if flag == "--manifest":
            manifest = value
        elif flag == "--old":
            old = value
        elif flag == "--new":
            new = value
        elif flag == "--format":
            format = value
        elif flag == "--output":
            output = value
        elif flag == "--rounds" and compute:
            rounds = int(value)
        else:
            raise ValueError("unknown argument: " + flag)
        index += 2
    if not manifest or not old or not new:
        raise ValueError("expected --manifest SUITE.json --old OLD.json --new NEW.json")
    if format != "github" and format != "diff":
        raise ValueError("format must be github or diff")
    if rounds < 0:
        raise ValueError("rounds must be nonnegative")
    return Options(manifest, old, new, format, force_summary_table, output, rounds)


def render_report(inputs: Inputs, format: str, force_summary_table: bool) -> str:
    expected = collect_expected_diagnostics(inputs.sources)
    old = collect_ty_diagnostics(inputs.old, "old", inputs.paths)
    new = collect_ty_diagnostics(inputs.new, "new", inputs.paths)
    grouped = group_diagnostics_by_key(old, new, expected)
    file_stats = collect_file_stats(grouped, inputs.manifest.test_files)
    base_url = f"https://github.com/python/typing/blob/{inputs.manifest.suite_revision}/conformance/"
    sections = [
        render_summary(grouped, file_stats, base_url, force_summary_table=force_summary_table),
        render_file_stats_table(file_stats, base_url),
        render_test_cases(grouped, base_url, format=format),
    ]
    return "\n\n".join(section for section in sections if section)


def main(arguments: list[str]) -> None:
    options = parse_options(arguments)
    inputs = read_inputs(options.manifest, options.old, options.new)
    rendered = render_report(inputs, options.format, options.force_summary_table)
    output = options.output
    if output is None:
        print(rendered)
    else:
        Path(str(output)).write_text(rendered, encoding="utf-8")


def compute_main(arguments: list[str]) -> None:
    options = parse_options(arguments, compute=True)
    inputs = read_inputs(options.manifest, options.old, options.new)
    checksum = 0
    rendered = ""
    for _ in range(options.rounds):
        rendered = render_report(inputs, options.format, options.force_summary_table)
        checksum += len(rendered)
    output = options.output
    if output is not None:
        Path(str(output)).write_text(rendered, encoding="utf-8")
    print(checksum)
