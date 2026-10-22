"""Complete report rendering adapted from the pinned Ruff source in upstream/."""
from pathlib import Path
from conformance_report.model import DiagnosticEntry, FileStats, Statistics, TestCase, TyDiagnostic, compute_stats, diagnostic_text

def signed_int(value: int) -> str:
    return '+' + str(value) if value >= 0 else str(value)

def signed_percent(value: float) -> str:
    text = f'{value:.2%}'
    return text if text.startswith('-') else '+' + text

def classification_title(classification: str, verb: str) -> str:
    if classification == 'true_positive':
        return f'True positives {verb}'
    if classification == 'false_positive':
        return f'False positives {verb}'
    if classification == 'true_negative':
        return f'True negatives {verb}'
    return f'False negatives {verb}'
GITHUB_HEADER = ['<table>', '<tr>', '<th>Test case</th>', '<th>Diff</th>', '</tr>']
GITHUB_FOOTER = ['</table>']
SUMMARY_NOTE = '\n    Each test case represents one expected error annotation or a group of annotations\n    sharing a tag. Counts are per test case, not per diagnostic — multiple diagnostics\n    on the same line count as one. Required annotations (`E`) are true positives when\n    ty flags the expected location and false negatives when it does not. Optional\n    annotations (`E?`) are true positives when flagged but true negatives (not false\n    negatives) when not. Tagged annotations (`E[tag]`) require ty to flag exactly one\n    of the tagged lines; tagged multi-annotations (`E[tag+]`) allow any number up to\n    the tag count. Flagging unexpected locations counts as a false positive.\n'
TITLE_PRIORITY: dict[str, int] = {'True positives added': 0, 'False positives removed': 1, 'True positives changed': 2, 'False positives changed': 3, 'False positives added': 4, 'True positives removed': 5, 'Optional Diagnostics Added': 6, 'Optional Diagnostics Removed': 7, 'Optional Diagnostics Changed': 8}

def fmt(old: int, new: int, *, greater_is_better: bool=True) -> str:
    if old == new:
        return str(new)
    diff = new - old
    improved = diff > 0 if greater_is_better else diff < 0
    indicator = ' ✅' if improved else ' ❌'
    return f'{new} ({signed_int(diff)}){indicator}'

def format_metric(diff: float, old: float, new: float) -> str:
    if diff > 0:
        return f'increased from <b>{old:.2%}</b> to <b>{new:.2%}</b>'
    if diff < 0:
        return f'decreased from <b>{old:.2%}</b> to <b>{new:.2%}</b>'
    return f'held steady at <b>{old:.2%}</b>'

def format_int_metric(diff: int, old: int, new: int, total: int) -> str:
    if diff > 0:
        return f'improved from <b>{old}/{total}</b> to <b>{new}/{total}</b>'
    if diff < 0:
        return f'regressed from <b>{old}/{total}</b> to <b>{new}/{total}</b>'
    return f'held steady at <b>{old}/{total}</b>'

def render_html_diff_row(tc: TestCase, base_url: str, *, source: str | None) -> list[str]:
    """Render a single HTML <tr> with a test-case link and a markdown diff block."""
    all_diags = [*tc.old, *tc.new] if source is None else tc.diagnostics_by_source(source)
    if all_diags:
        min_line = all_diags[0].location.positions.begin.line
        max_line = min_line
        for diagnostic in all_diags:
            line = diagnostic.location.positions.begin.line
            if line < min_line:
                min_line = line
            if line > max_line:
                max_line = line
        filename = Path(all_diags[0].location.path).name
        if min_line == max_line:
            url = f'{base_url}tests/{filename}#L{min_line}'
            display = f'{filename}:{min_line}'
        else:
            url = f'{base_url}tests/{filename}#L{min_line}-L{max_line}'
            display = f'{filename}:{min_line}:{max_line}'
        location = f'[{display}]({url})'
    else:
        location = tc.key
    diff_lines: list[str] = []
    if source is None:
        for d in tc.old:
            diff_lines.append(f'-{d.severity_for_display}[{d.check_name}] {d.description}')
        for d in tc.new:
            diff_lines.append(f'+{d.severity_for_display}[{d.check_name}] {d.description}')
    else:
        sign = '-' if source == 'old' else '+'
        for d in tc.diagnostics_by_source(source):
            diff_lines.append(f'{sign}{d.severity_for_display}[{d.check_name}] {d.description}')
    return ['', '<tr>', '<td>', '', location, '', '</td>', '', '<td>', '', '```diff', *diff_lines, '```', '', '</td>', '</tr>']

def render_diff_row(diagnostics: list[TyDiagnostic], *, removed: bool=False) -> str:
    sign = '-' if removed else '+'
    return '\n'.join((f'{sign} {diagnostic_text(d)}' for d in diagnostics))

def collect_diagnostic_entries(test_cases: list[TestCase]) -> list[DiagnosticEntry]:
    """Classify each changed test case and assign it to a titled section."""
    entries: list[DiagnosticEntry] = []
    for tc in test_cases:
        change = tc.change
        if change == 'unchanged':
            continue
        if tc.optional:
            if change == 'added':
                entries.append(DiagnosticEntry(title='Optional Diagnostics Added', test_case=tc, source='new'))
            elif change == 'removed':
                entries.append(DiagnosticEntry(title='Optional Diagnostics Removed', test_case=tc, source='old'))
            elif change == 'changed':
                entries.append(DiagnosticEntry(title='Optional Diagnostics Changed', test_case=tc, source=None))
            else:
                raise AssertionError('unreachable diagnostic change')
        elif change == 'added':
            new_class = tc.classify('new')
            entries.append(DiagnosticEntry(title=classification_title(new_class, 'added'), test_case=tc, source='new'))
        elif change == 'removed':
            old_class = tc.classify('old')
            entries.append(DiagnosticEntry(title=classification_title(old_class, 'removed'), test_case=tc, source='old'))
        elif change == 'changed':
            old_class = tc.classify('old')
            new_class = tc.classify('new')
            if old_class == new_class:
                entries.append(DiagnosticEntry(title=classification_title(new_class, 'changed'), test_case=tc, source=None))
            else:
                entries.append(DiagnosticEntry(title=classification_title(old_class, 'removed'), test_case=tc, source='old'))
                entries.append(DiagnosticEntry(title=classification_title(new_class, 'added'), test_case=tc, source='new'))
        else:
            raise AssertionError('unreachable diagnostic change')
    entries = sorted(entries, key=lambda e: (TITLE_PRIORITY.get(e.title, 99), e.title, e.test_case.key))
    return entries

def render_test_cases(test_cases: list[TestCase], base_url: str, *, format: str='diff') -> str:
    entries = collect_diagnostic_entries(test_cases)
    if not entries:
        return ''
    lines: list[str] = []
    position = 0
    while position < len(entries):
        title = entries[position].title
        end = position + 1
        while end < len(entries) and entries[end].title == title:
            end += 1
        group_list = entries[position:end]
        n = len(group_list)
        lines.append(f'### {title} ({n})')
        lines.extend(['', '<details>', f"<summary>{n} {('diagnostic' if n == 1 else 'diagnostics')}</summary>", ''])
        if format == 'diff':
            lines.append('```diff')
        else:
            lines.extend(GITHUB_HEADER)
        for entry in group_list:
            tc, source = (entry.test_case, entry.source)
            if source is None:
                if format == 'diff':
                    lines.append(render_diff_row(tc.old, removed=True))
                    lines.append(render_diff_row(tc.new, removed=False))
                else:
                    lines.extend(render_html_diff_row(tc, base_url, source=None))
            elif format == 'diff':
                lines.append(render_diff_row(tc.diagnostics_by_source(source), removed=source == 'old'))
            else:
                lines.extend(render_html_diff_row(tc, base_url, source=source))
        if format == 'diff':
            lines.append('```')
        else:
            lines.extend(GITHUB_FOOTER)
        lines.extend(['</details>', ''])
        position = end
    return '\n'.join(lines)

def render_file_stats_table(file_stats: list[FileStats], base_url: str) -> str:
    """Render a per-file breakdown showing only files whose TP/FP/FN counts changed."""
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
        passing += 1 if fs.new_passes else 0
    changed = [fs for fs in file_stats if fs.total_change > 0]
    if not changed:
        return ''
    changed = sorted(changed, key=lambda fs: (-fs.total_change, Path(fs.path).name))
    rows: list[str] = []
    for fs in changed:
        if fs.new_passes and (not fs.old_passes):
            status = '✅ Newly Passing 🎉'
        elif fs.old_passes and (not fs.new_passes):
            status = '❌ Newly Failing ☹️'
        elif fs.new_passes:
            status = '✅ Still Passing'
        else:
            old_errors = fs.old.false_positives + fs.old.false_negatives
            new_errors = fs.new.false_positives + fs.new.false_negatives
            if new_errors < old_errors:
                status = '📈 Improving'
            elif new_errors > old_errors:
                status = '📉 Regressing'
            else:
                status = '➡️ Neutral'
        url = base_url + f'tests/{Path(fs.path).name}'
        rows.append(f'| [{Path(fs.path).name}]({url}) | {fmt(fs.old.true_positives, fs.new.true_positives, greater_is_better=True)} | {fmt(fs.old.false_positives, fs.new.false_positives, greater_is_better=False)} | {fmt(fs.old.false_negatives, fs.new.false_negatives, greater_is_better=False)} | {status} |')
    totals_row = f'| **Total (all files)** | **{fmt(old_totals.true_positives, new_totals.true_positives, greater_is_better=True)}** | **{fmt(old_totals.false_positives, new_totals.false_positives, greater_is_better=False)}** | **{fmt(old_totals.false_negatives, new_totals.false_negatives, greater_is_better=False)}** | {passing}/{total_files} |'
    lines = ['### Test file breakdown', '', '<details>', f"<summary>{len(changed)} file{('s' if len(changed) != 1 else '')} altered</summary>", '', '| File | True Positives | False Positives | False Negatives | Status |', '|------|----|----|----|--------|', *rows, totals_row, '', '</details>', '']
    return '\n'.join(lines)

def diff_format(diff: float, *, greater_is_better: bool=True, neutral: bool=False) -> str:
    if diff == 0:
        return ''
    increased = diff > 0
    good = ' (✅)' if not neutral else ''
    bad = ' (❌)' if not neutral else ''
    up = '⏫'
    down = '⏬'
    match (greater_is_better, increased):
        case (True, True):
            return f'{up}{good}'
        case (False, True):
            return f'{up}{bad}'
        case (True, False):
            return f'{down}{bad}'
        case (False, False):
            return f'{down}{good}'
        case _:
            raise AssertionError('unreachable metric direction')

def render_summary(test_cases: list[TestCase], file_stats: list[FileStats], base_url: str, *, force_summary_table: bool) -> str:
    old = compute_stats(test_cases, 'old')
    new = compute_stats(test_cases, 'new')
    old_files_passing = sum((1 if fs.old_passes else 0 for fs in file_stats))
    new_files_passing = sum((1 if fs.new_passes else 0 for fs in file_stats))
    total_files = len(file_stats)
    files_passing_change = new_files_passing - old_files_passing
    assert new.true_positives > 0, 'Expected ty to have at least one true positive.'
    precision_change = new.precision - old.precision
    recall_change = new.recall - old.recall
    true_pos_change = new.true_positives - old.true_positives
    false_pos_change = new.false_positives - old.false_positives
    false_neg_change = new.false_negatives - old.false_negatives
    total_change = new.total_diagnostics - old.total_diagnostics
    summary_paragraph = f'The percentage of diagnostics emitted that were expected errors {format_metric(precision_change, old.precision, new.precision)}. The percentage of expected errors that received a diagnostic {format_metric(recall_change, old.recall, new.recall)}. The number of fully passing files {format_int_metric(files_passing_change, old_files_passing, new_files_passing, total_files)}.'
    base_header = f'[Typing conformance results]({base_url})'
    if not force_summary_table and all((diag.change == 'unchanged' for diag in test_cases)):
        return f'\n## {base_header}\n\n### No changes detected ✅\n\n<details>\n<summary>Current numbers</summary>\n\n<br>\n{summary_paragraph}\n\n</details>\n'
    true_pos_diff = diff_format(float(true_pos_change), greater_is_better=True)
    false_pos_diff = diff_format(float(false_pos_change), greater_is_better=False)
    false_neg_diff = diff_format(float(false_neg_change), greater_is_better=False)
    precision_diff = diff_format(precision_change, greater_is_better=True)
    recall_diff = diff_format(recall_change, greater_is_better=True)
    total_diff = diff_format(float(total_change), neutral=True)
    passing_diff = diff_format(float(files_passing_change), greater_is_better=True)
    if precision_change > 0 and recall_change >= 0 and (files_passing_change >= 0) or (recall_change > 0 and precision_change >= 0 and (files_passing_change >= 0)) or (files_passing_change > 0 and precision_change >= 0 and (recall_change >= 0)):
        header = f'{base_header} improved 🎉'
    elif precision_change < 0 and recall_change <= 0 and (files_passing_change <= 0) or (recall_change < 0 and precision_change <= 0 and (files_passing_change <= 0)) or (files_passing_change < 0 and precision_change <= 0 and (recall_change <= 0)):
        header = f'{base_header} regressed ❌'
    else:
        header = base_header
    summary_note = ' '.join(SUMMARY_NOTE.split())
    return f'\n## {header}\n\n{summary_paragraph}\n\n### Summary\n\n<details>\n<summary>How are test cases classified?</summary>\n\n<br>\n\n{summary_note}\n\n</details>\n\n| Metric | Old | New | Diff | Outcome |\n|--------|-----|-----|------|---------|\n| True Positives  | {old.true_positives} | {new.true_positives} | {signed_int(true_pos_change)} | {true_pos_diff} |\n| False Positives | {old.false_positives} | {new.false_positives} | {signed_int(false_pos_change)} | {false_pos_diff} |\n| False Negatives | {old.false_negatives} | {new.false_negatives} | {signed_int(false_neg_change)} | {false_neg_diff} |\n| Total Diagnostics | {old.total_diagnostics} | {new.total_diagnostics} | {signed_int(total_change)} | {total_diff} |\n| Precision | {old.precision:.2%} | {new.precision:.2%} | {signed_percent(precision_change)} | {precision_diff} |\n| Recall | {old.recall:.2%} | {new.recall:.2%} | {signed_percent(recall_change)} | {recall_diff} |\n| Passing Files | {old_files_passing}/{total_files} | {new_files_passing}/{total_files} | {signed_int(files_passing_change)} | {passing_diff} |\n\n'
