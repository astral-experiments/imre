"""Complete report rendering adapted from the pinned Ruff source in upstream/."""
from itertools import groupby
from textwrap import dedent
from typing import Literal
from typing import assert_never
from conformance_report.model import Change, DiagnosticEntry, FileStats, Source, Statistics, TestCase, TyDiagnostic, compute_stats

GITHUB_HEADER = ['<table>', '<tr>', '<th>Test case</th>', '<th>Diff</th>', '</tr>']
GITHUB_FOOTER = ['</table>']
SUMMARY_NOTE = '\n    Each test case represents one expected error annotation or a group of annotations\n    sharing a tag. Counts are per test case, not per diagnostic — multiple diagnostics\n    on the same line count as one. Required annotations (`E`) are true positives when\n    ty flags the expected location and false negatives when it does not. Optional\n    annotations (`E?`) are true positives when flagged but true negatives (not false\n    negatives) when not. Tagged annotations (`E[tag]`) require ty to flag exactly one\n    of the tagged lines; tagged multi-annotations (`E[tag+]`) allow any number up to\n    the tag count. Flagging unexpected locations counts as a false positive.\n'
TITLE_PRIORITY: dict[str, int] = {'True positives added': 0, 'False positives removed': 1, 'True positives changed': 2, 'False positives changed': 3, 'False positives added': 4, 'True positives removed': 5, 'Optional Diagnostics Added': 6, 'Optional Diagnostics Removed': 7, 'Optional Diagnostics Changed': 8}

def render_diff_row(diagnostics: list[TyDiagnostic], *, removed: bool = False) -> str:
    sign = "-" if removed else "+"
    return "\n".join(f"{sign} {d}" for d in diagnostics)

def collect_diagnostic_entries(test_cases: list[TestCase]) -> list[DiagnosticEntry]:
    """Classify each changed test case and assign it to a titled section."""
    entries: list[DiagnosticEntry] = []

    for tc in test_cases:
        change = tc.change
        if change == Change.UNCHANGED:
            continue

        if tc.optional:
            if change == Change.ADDED:
                entries.append(
                    DiagnosticEntry(
                        title=Change.ADDED.into_title(), test_case=tc, source=Source.NEW
                    )
                )
            elif change == Change.REMOVED:
                entries.append(
                    DiagnosticEntry(
                        title=Change.REMOVED.into_title(),
                        test_case=tc,
                        source=Source.OLD,
                    )
                )
            elif change == Change.CHANGED:
                entries.append(
                    DiagnosticEntry(
                        title=Change.CHANGED.into_title(), test_case=tc, source=None
                    )
                )
            else:
                assert_never(change)
        else:
            if change == Change.ADDED:
                new_class = tc.classify(Source.NEW)
                entries.append(
                    DiagnosticEntry(
                        title=new_class.into_title(verb="added"),
                        test_case=tc,
                        source=Source.NEW,
                    )
                )
            elif change == Change.REMOVED:
                old_class = tc.classify(Source.OLD)
                entries.append(
                    DiagnosticEntry(
                        title=old_class.into_title(verb="removed"),
                        test_case=tc,
                        source=Source.OLD,
                    )
                )
            elif change == Change.CHANGED:
                old_class = tc.classify(Source.OLD)
                new_class = tc.classify(Source.NEW)
                if old_class == new_class:
                    # Same classification but different diagnostics: one "changed" section.
                    entries.append(
                        DiagnosticEntry(
                            title=new_class.into_title(verb="changed"),
                            test_case=tc,
                            source=None,
                        )
                    )
                else:
                    # Classification changed: split into separate removed/added sections.
                    entries.append(
                        DiagnosticEntry(
                            title=old_class.into_title(verb="removed"),
                            test_case=tc,
                            source=Source.OLD,
                        )
                    )
                    entries.append(
                        DiagnosticEntry(
                            title=new_class.into_title(verb="added"),
                            test_case=tc,
                            source=Source.NEW,
                        )
                    )
            else:
                assert_never(change)

    entries.sort(
        key=lambda e: (TITLE_PRIORITY.get(e.title, 99), e.title, e.test_case.key)
    )
    return entries

def render_cases(test_cases: list[TestCase], CONFORMANCE_DIR_WITH_README: str, *, format: str = "diff") -> str:
    CONFORMANCE_URL = CONFORMANCE_DIR_WITH_README + "tests/{filename}#L{line}"

    def render_html_diff_row(tc: TestCase, *, source: Source | None) -> list[str]:
        """Render a single HTML <tr> with a test-case link and a markdown diff block."""
        all_diags = tc.old + tc.new if source is None else tc.diagnostics_by_source(source)

        if all_diags:
            min_line = min(d.location.positions.begin.line for d in all_diags)
            max_line = max(d.location.positions.begin.line for d in all_diags)
            filename = all_diags[0].location.path.name
            if min_line == max_line:
                url = CONFORMANCE_URL.format(filename=filename, line=min_line)
                display = f"{filename}:{min_line}"
            else:
                url = (
                    f"{CONFORMANCE_DIR_WITH_README}tests/{filename}#L{min_line}-L{max_line}"
                )
                display = f"{filename}:{min_line}:{max_line}"
            location = f"[{display}]({url})"
        else:
            location = tc.key

        diff_lines = []
        if source is None:
            for d in tc.old:
                diff_lines.append(
                    f"-{d.severity_for_display}[{d.check_name}] {d.description}"
                )
            for d in tc.new:
                diff_lines.append(
                    f"+{d.severity_for_display}[{d.check_name}] {d.description}"
                )
        else:
            sign = "-" if source == Source.OLD else "+"
            for d in tc.diagnostics_by_source(source):
                diff_lines.append(
                    f"{sign}{d.severity_for_display}[{d.check_name}] {d.description}"
                )

        return [
            "",
            "<tr>",
            "<td>",
            "",
            location,
            "",
            "</td>",
            "",
            "<td>",
            "",
            "```diff",
            *diff_lines,
            "```",
            "",
            "</td>",
            "</tr>",
        ]

    def render_test_cases(
        test_cases: list[TestCase],
        *,
        format: Literal["diff", "github"] = "diff",
    ) -> str:
        entries = collect_diagnostic_entries(test_cases)
        if not entries:
            return ""

        lines = []
        for title, group in groupby(entries, key=lambda e: e.title):
            group_list = list(group)
            n = len(group_list)

            lines.append(f"### {title} ({n})")
            lines.extend(
                [
                    "",
                    "<details>",
                    f"<summary>{n} {'diagnostic' if n == 1 else 'diagnostics'}</summary>",
                    "",
                ]
            )

            if format == "diff":
                lines.append("```diff")
            else:
                lines.extend(GITHUB_HEADER)

            for entry in group_list:
                tc, source = entry.test_case, entry.source
                if source is None:
                    if format == "diff":
                        lines.append(render_diff_row(tc.old, removed=True))
                        lines.append(render_diff_row(tc.new, removed=False))
                    else:
                        lines.extend(render_html_diff_row(tc, source=None))
                else:
                    if format == "diff":
                        lines.append(
                            render_diff_row(
                                tc.diagnostics_by_source(source),
                                removed=source == Source.OLD,
                            )
                        )
                    else:
                        lines.extend(render_html_diff_row(tc, source=source))

            if format == "diff":
                lines.append("```")
            else:
                lines.extend(GITHUB_FOOTER)
            lines.extend(["</details>", ""])

        return "\n".join(lines)

    if format == "diff":
        return render_test_cases(test_cases)
    return render_test_cases(test_cases, format="github")


def render_file_stats(file_stats: list[FileStats], CONFORMANCE_DIR_WITH_README: str) -> str:
    # Bind Ruff's report URL to this checkout for the duration of the call.
    def render_file_stats_table(file_stats: list[FileStats]) -> str:
        """Render a per-file breakdown showing only files whose TP/FP/FN counts changed."""

        def fmt(old: int, new: int, *, greater_is_better: bool = True) -> str:
            if old == new:
                return str(new)
            diff = new - old
            improved = diff > 0 if greater_is_better else diff < 0
            indicator = " ✅" if improved else " ❌"
            return f"{new} ({diff:+}){indicator}"

        # Collect totals across all files regardless of change.
        old_totals = Statistics()
        new_totals = Statistics()
        passing = 0
        total_files = len(file_stats)

        for fs in file_stats:
            old_totals.true_positives += fs.old.true_positives
            old_totals.false_positives += fs.old.false_positives
            old_totals.false_negatives += fs.old.false_negatives
            new_totals.true_positives += fs.new.true_positives
            new_totals.false_positives += fs.new.false_positives
            new_totals.false_negatives += fs.new.false_negatives
            passing += fs.new_passes

        changed = [fs for fs in file_stats if fs.total_change > 0]
        if not changed:
            return ""

        changed.sort(key=lambda fs: (-fs.total_change, fs.path.name))

        rows = []
        for fs in changed:
            if fs.new_passes and not fs.old_passes:
                status = "✅ Newly Passing 🎉"
            elif fs.old_passes and not fs.new_passes:
                status = "❌ Newly Failing ☹️"
            elif fs.new_passes:
                status = "✅ Still Passing"
            else:
                old_errors = fs.old.false_positives + fs.old.false_negatives
                new_errors = fs.new.false_positives + fs.new.false_negatives
                if new_errors < old_errors:
                    status = "📈 Improving"
                elif new_errors > old_errors:
                    status = "📉 Regressing"
                else:
                    status = "➡️ Neutral"
            url = CONFORMANCE_DIR_WITH_README + f"tests/{fs.path.name}"
            rows.append(
                f"| [{fs.path.name}]({url})"
                f" | {fmt(fs.old.true_positives, fs.new.true_positives, greater_is_better=True)}"
                f" | {fmt(fs.old.false_positives, fs.new.false_positives, greater_is_better=False)}"
                f" | {fmt(fs.old.false_negatives, fs.new.false_negatives, greater_is_better=False)}"
                f" | {status} |"
            )

        totals_row = (
            f"| **Total (all files)**"
            f" | **{fmt(old_totals.true_positives, new_totals.true_positives, greater_is_better=True)}**"
            f" | **{fmt(old_totals.false_positives, new_totals.false_positives, greater_is_better=False)}**"
            f" | **{fmt(old_totals.false_negatives, new_totals.false_negatives, greater_is_better=False)}**"
            f" | {passing}/{total_files} |"
        )

        lines = [
            "### Test file breakdown",
            "",
            "<details>",
            f"<summary>{len(changed)} file{'s' if len(changed) != 1 else ''} altered</summary>",
            "",
            "| File | True Positives | False Positives | False Negatives | Status |",
            "|------|----|----|----|--------|",
            *rows,
            totals_row,
            "",
            "</details>",
            "",
        ]
        return "\n".join(lines)

    return render_file_stats_table(file_stats)

def diff_format(
    diff: float,
    *,
    greater_is_better: bool = True,
    neutral: bool = False,
) -> str:
    if diff == 0:
        return ""

    increased = diff > 0
    good = " (✅)" if not neutral else ""
    bad = " (❌)" if not neutral else ""
    up = "⏫"
    down = "⏬"

    match (greater_is_better, increased):
        case (True, True):
            return f"{up}{good}"
        case (False, True):
            return f"{up}{bad}"
        case (True, False):
            return f"{down}{bad}"
        case (False, False):
            return f"{down}{good}"
        case _:
            assert_never((greater_is_better, increased))


def summary_for_checkout(
    test_cases: list[TestCase], file_stats: list[FileStats], CONFORMANCE_DIR_WITH_README: str,
    *, force_summary_table: bool,
) -> str:
    def render_summary(
        test_cases: list[TestCase],
        file_stats: list[FileStats],
        *,
        force_summary_table: bool,
    ) -> str:
        def format_metric(diff: float, old: float, new: float) -> str:
            if diff > 0:
                return f"increased from <b>{old:.2%}</b> to <b>{new:.2%}</b>"
            if diff < 0:
                return f"decreased from <b>{old:.2%}</b> to <b>{new:.2%}</b>"
            return f"held steady at <b>{old:.2%}</b>"

        def format_int_metric(diff: int, old: int, new: int, total: int) -> str:
            if diff > 0:
                return f"improved from <b>{old}/{total}</b> to <b>{new}/{total}</b>"
            if diff < 0:
                return f"regressed from <b>{old}/{total}</b> to <b>{new}/{total}</b>"
            return f"held steady at <b>{old}/{total}</b>"

        old = compute_stats(test_cases, Source.OLD)
        new = compute_stats(test_cases, Source.NEW)

        old_files_passing = sum(fs.old_passes for fs in file_stats)
        new_files_passing = sum(fs.new_passes for fs in file_stats)
        total_files = len(file_stats)
        files_passing_change = new_files_passing - old_files_passing

        assert new.true_positives > 0, (
            "Expected ty to have at least one true positive.\n"
            f"Sample of grouped diagnostics: {test_cases[:5]}"
        )

        precision_change = new.precision - old.precision
        recall_change = new.recall - old.recall
        true_pos_change = new.true_positives - old.true_positives
        false_pos_change = new.false_positives - old.false_positives
        false_neg_change = new.false_negatives - old.false_negatives
        total_change = new.total_diagnostics - old.total_diagnostics

        summary_paragraph = (
            f"The percentage of diagnostics emitted that were expected errors "
            f"{format_metric(precision_change, old.precision, new.precision)}. "
            f"The percentage of expected errors that received a diagnostic "
            f"{format_metric(recall_change, old.recall, new.recall)}. "
            f"The number of fully passing files "
            f"{format_int_metric(files_passing_change, old_files_passing, new_files_passing, total_files)}."
        )

        base_header = f"[Typing conformance results]({CONFORMANCE_DIR_WITH_README})"

        if not force_summary_table and all(
            diag.change is Change.UNCHANGED for diag in test_cases
        ):
            return dedent(
                f"""
            ## {base_header}

            ### No changes detected ✅

            <details>
            <summary>Current numbers</summary>

            <br>
            {summary_paragraph}

            </details>
            """
            )

        true_pos_diff = diff_format(true_pos_change, greater_is_better=True)
        false_pos_diff = diff_format(false_pos_change, greater_is_better=False)
        false_neg_diff = diff_format(false_neg_change, greater_is_better=False)
        precision_diff = diff_format(precision_change, greater_is_better=True)
        recall_diff = diff_format(recall_change, greater_is_better=True)
        total_diff = diff_format(total_change, neutral=True)
        passing_diff = diff_format(files_passing_change, greater_is_better=True)

        if (
            (precision_change > 0 and recall_change >= 0 and files_passing_change >= 0)
            or (recall_change > 0 and precision_change >= 0 and files_passing_change >= 0)
            or (files_passing_change > 0 and precision_change >= 0 and recall_change >= 0)
        ):
            header = f"{base_header} improved 🎉"
        elif (
            (precision_change < 0 and recall_change <= 0 and files_passing_change <= 0)
            or (recall_change < 0 and precision_change <= 0 and files_passing_change <= 0)
            or (files_passing_change < 0 and precision_change <= 0 and recall_change <= 0)
        ):
            header = f"{base_header} regressed ❌"
        else:
            header = base_header

        summary_note = " ".join(SUMMARY_NOTE.split())

        return dedent(
            f"""
        ## {header}

        {summary_paragraph}

        ### Summary

        <details>
        <summary>How are test cases classified?</summary>

        <br>

        {summary_note}

        </details>

        | Metric | Old | New | Diff | Outcome |
        |--------|-----|-----|------|---------|
        | True Positives  | {old.true_positives} | {new.true_positives} | {true_pos_change:+} | {true_pos_diff} |
        | False Positives | {old.false_positives} | {new.false_positives} | {false_pos_change:+} | {false_pos_diff} |
        | False Negatives | {old.false_negatives} | {new.false_negatives} | {false_neg_change:+} | {false_neg_diff} |
        | Total Diagnostics | {old.total_diagnostics} | {new.total_diagnostics} | {total_change:+} | {total_diff} |
        | Precision | {old.precision:.2%} | {new.precision:.2%} | {precision_change:+.2%} | {precision_diff} |
        | Recall | {old.recall:.2%} | {new.recall:.2%} | {recall_change:+.2%} | {recall_diff} |
        | Passing Files | {old_files_passing}/{total_files} | {new_files_passing}/{total_files} | {files_passing_change:+} | {passing_diff} |

        """
        )

    return render_summary(test_cases, file_stats, force_summary_table=force_summary_table)
