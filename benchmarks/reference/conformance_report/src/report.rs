use crate::Result;
use crate::model::{
    Change, Diagnostic, FileStats, Side, Statistics, TestCase, compute_stats, filename,
};

mod assertion;

const SUMMARY_NOTE: &str = "Each test case represents one expected error annotation or a group of annotations sharing a tag. Counts are per test case, not per diagnostic — multiple diagnostics on the same line count as one. Required annotations (`E`) are true positives when ty flags the expected location and false negatives when it does not. Optional annotations (`E?`) are true positives when flagged but true negatives (not false negatives) when not. Tagged annotations (`E[tag]`) require ty to flag exactly one of the tagged lines; tagged multi-annotations (`E[tag+]`) allow any number up to the tag count. Flagging unexpected locations counts as a false positive.";

pub(crate) struct Entry<'a> {
    pub(crate) title: String,
    pub(crate) case: &'a TestCase,
    pub(crate) source: Option<Side>,
}

fn priority(title: &str) -> u8 {
    match title {
        "True positives added" => 0,
        "False positives removed" => 1,
        "True positives changed" => 2,
        "False positives changed" => 3,
        "False positives added" => 4,
        "True positives removed" => 5,
        "Optional Diagnostics Added" => 6,
        "Optional Diagnostics Removed" => 7,
        "Optional Diagnostics Changed" => 8,
        _ => 99,
    }
}

pub(crate) fn entries(cases: &[TestCase]) -> Vec<Entry<'_>> {
    let mut result = Vec::new();
    for case in cases {
        let change = case.change();
        if change == Change::Unchanged {
            continue;
        }
        if case.optional() {
            let (title, source) = match change {
                Change::Added => ("Optional Diagnostics Added", Some(Side::New)),
                Change::Removed => ("Optional Diagnostics Removed", Some(Side::Old)),
                Change::Changed => ("Optional Diagnostics Changed", None),
                Change::Unchanged => unreachable!(),
            };
            result.push(Entry {
                title: title.to_owned(),
                case,
                source,
            });
        } else {
            match change {
                Change::Added => result.push(Entry {
                    title: case.classify(Side::New).title("added"),
                    case,
                    source: Some(Side::New),
                }),
                Change::Removed => result.push(Entry {
                    title: case.classify(Side::Old).title("removed"),
                    case,
                    source: Some(Side::Old),
                }),
                Change::Changed => {
                    let old = case.classify(Side::Old);
                    let new = case.classify(Side::New);
                    if old == new {
                        result.push(Entry {
                            title: new.title("changed"),
                            case,
                            source: None,
                        });
                    } else {
                        result.push(Entry {
                            title: old.title("removed"),
                            case,
                            source: Some(Side::Old),
                        });
                        result.push(Entry {
                            title: new.title("added"),
                            case,
                            source: Some(Side::New),
                        });
                    }
                }
                Change::Unchanged => unreachable!(),
            }
        }
    }
    result.sort_by(|left, right| {
        (priority(&left.title), &left.title, &left.case.key).cmp(&(
            priority(&right.title),
            &right.title,
            &right.case.key,
        ))
    });
    result
}

fn diff_row(diagnostics: &[Diagnostic], removed: bool) -> String {
    let sign = if removed { '-' } else { '+' };
    diagnostics
        .iter()
        .map(|diagnostic| format!("{sign} {}", diagnostic.display()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn html_row(case: &TestCase, base: &str, source: Option<Side>) -> Vec<String> {
    let diagnostics: Vec<_> = if let Some(source) = source {
        case.diagnostics(source).iter().collect()
    } else {
        case.old.iter().chain(case.new.iter()).collect()
    };
    let location = if let Some(first) = diagnostics.first() {
        let mut minimum = &first.positions.begin.line;
        let mut maximum = minimum;
        for diagnostic in &diagnostics {
            let line = &diagnostic.positions.begin.line;
            if line < minimum {
                minimum = line;
            }
            if line > maximum {
                maximum = line;
            }
        }
        let name = filename(&first.path);
        if minimum == maximum {
            format!("[{name}:{minimum}]({base}tests/{name}#L{minimum})")
        } else {
            format!("[{name}:{minimum}:{maximum}]({base}tests/{name}#L{minimum}-L{maximum})")
        }
    } else {
        case.key.clone()
    };
    let mut lines: Vec<String> = ["", "<tr>", "<td>", ""]
        .into_iter()
        .map(str::to_owned)
        .collect();
    lines.push(location);
    lines.extend(["", "</td>", "", "<td>", "", "```diff"].map(str::to_owned));
    for (side, sign) in [(Side::Old, '-'), (Side::New, '+')] {
        if source.is_none() || source == Some(side) {
            lines.extend(case.diagnostics(side).iter().map(|diagnostic| {
                format!(
                    "{sign}{}[{}] {}",
                    diagnostic.severity_for_display(),
                    diagnostic.check_name,
                    diagnostic.description
                )
            }));
        }
    }
    lines.extend(["```", "", "</td>", "</tr>"].map(str::to_owned));
    lines
}

pub fn test_cases(cases: &[TestCase], base: &str, format: &str) -> String {
    let entries = entries(cases);
    let mut lines = Vec::new();
    let mut position = 0;
    while position < entries.len() {
        let title = &entries[position].title;
        let mut end = position + 1;
        while end < entries.len() && &entries[end].title == title {
            end += 1;
        }
        let count = end - position;
        lines.push(format!("### {title} ({count})"));
        lines.extend([
            String::new(),
            "<details>".into(),
            format!(
                "<summary>{count} {}</summary>",
                if count == 1 {
                    "diagnostic"
                } else {
                    "diagnostics"
                }
            ),
            String::new(),
        ]);
        if format == "diff" {
            lines.push("```diff".into());
        } else {
            lines.extend(
                [
                    "<table>",
                    "<tr>",
                    "<th>Test case</th>",
                    "<th>Diff</th>",
                    "</tr>",
                ]
                .map(str::to_owned),
            );
        }
        for entry in &entries[position..end] {
            if format == "diff" {
                if let Some(source) = entry.source {
                    lines.push(diff_row(
                        entry.case.diagnostics(source),
                        source == Side::Old,
                    ));
                } else {
                    lines.push(diff_row(&entry.case.old, true));
                    lines.push(diff_row(&entry.case.new, false));
                }
            } else {
                lines.extend(html_row(entry.case, base, entry.source));
            }
        }
        lines.push(if format == "diff" { "```" } else { "</table>" }.into());
        lines.extend(["</details>".into(), String::new()]);
        position = end;
    }
    lines.join("\n")
}

fn count_metric(old: i64, new: i64, greater_is_better: bool) -> String {
    if old == new {
        return new.to_string();
    }
    let difference = new - old;
    let improved = if greater_is_better {
        difference > 0
    } else {
        difference < 0
    };
    format!(
        "{new} ({difference:+}) {}",
        if improved { "✅" } else { "❌" }
    )
}

pub fn file_stats(files: &[FileStats], base: &str) -> String {
    let mut old = Statistics::default();
    let mut new = Statistics::default();
    let mut passing = 0;
    for file in files {
        old.true_positives += file.old.true_positives;
        old.false_positives += file.old.false_positives;
        old.false_negatives += file.old.false_negatives;
        new.true_positives += file.new.true_positives;
        new.false_positives += file.new.false_positives;
        new.false_negatives += file.new.false_negatives;
        passing += i64::from(file.new.passes());
    }
    let mut changed: Vec<_> = files
        .iter()
        .filter(|file| file.total_change() > 0)
        .collect();
    if changed.is_empty() {
        return String::new();
    }
    changed.sort_by_key(|file| (-file.total_change(), filename(&file.path)));
    let mut lines = vec![
        "### Test file breakdown".into(),
        String::new(),
        "<details>".into(),
        format!(
            "<summary>{} file{} altered</summary>",
            changed.len(),
            if changed.len() != 1 { "s" } else { "" }
        ),
        String::new(),
        "| File | True Positives | False Positives | False Negatives | Status |".into(),
        "|------|----|----|----|--------|".into(),
    ];
    for file in &changed {
        let status = if file.new.passes() && !file.old.passes() {
            "✅ Newly Passing 🎉"
        } else if file.old.passes() && !file.new.passes() {
            "❌ Newly Failing ☹️"
        } else if file.new.passes() {
            "✅ Still Passing"
        } else {
            let old_errors = file.old.false_positives + file.old.false_negatives;
            let new_errors = file.new.false_positives + file.new.false_negatives;
            if new_errors < old_errors {
                "📈 Improving"
            } else if new_errors > old_errors {
                "📉 Regressing"
            } else {
                "➡️ Neutral"
            }
        };
        let name = filename(&file.path);
        lines.push(format!(
            "| [{name}]({base}tests/{name}) | {} | {} | {} | {status} |",
            count_metric(file.old.true_positives, file.new.true_positives, true),
            count_metric(file.old.false_positives, file.new.false_positives, false),
            count_metric(file.old.false_negatives, file.new.false_negatives, false)
        ));
    }
    lines.push(format!(
        "| **Total (all files)** | **{}** | **{}** | **{}** | {passing}/{} |",
        count_metric(old.true_positives, new.true_positives, true),
        count_metric(old.false_positives, new.false_positives, false),
        count_metric(old.false_negatives, new.false_negatives, false),
        files.len()
    ));
    lines.extend([String::new(), "</details>".into(), String::new()]);
    lines.join("\n")
}

fn percent(value: f64, signed: bool) -> String {
    if signed {
        format!("{:+.2}%", value * 100.0)
    } else {
        format!("{:.2}%", value * 100.0)
    }
}

fn metric(difference: f64, old: f64, new: f64) -> String {
    if difference > 0.0 {
        format!(
            "increased from <b>{}</b> to <b>{}</b>",
            percent(old, false),
            percent(new, false)
        )
    } else if difference < 0.0 {
        format!(
            "decreased from <b>{}</b> to <b>{}</b>",
            percent(old, false),
            percent(new, false)
        )
    } else {
        format!("held steady at <b>{}</b>", percent(old, false))
    }
}

fn integer_metric(difference: i64, old: i64, new: i64, total: usize) -> String {
    if difference > 0 {
        format!("improved from <b>{old}/{total}</b> to <b>{new}/{total}</b>")
    } else if difference < 0 {
        format!("regressed from <b>{old}/{total}</b> to <b>{new}/{total}</b>")
    } else {
        format!("held steady at <b>{old}/{total}</b>")
    }
}

fn outcome(difference: f64, greater_is_better: bool, neutral: bool) -> &'static str {
    if difference == 0.0 {
        return "";
    }
    let increased = difference > 0.0;
    if neutral {
        return if increased { "⏫" } else { "⏬" };
    }
    match (increased, increased == greater_is_better) {
        (true, true) => "⏫ (✅)",
        (true, false) => "⏫ (❌)",
        (false, true) => "⏬ (✅)",
        (false, false) => "⏬ (❌)",
    }
}

/// Ruff dedents after inserting the revision. Only the header can contain
/// interpolated newlines; every nonblank source line starts with `indent` spaces.
fn summary_template(header: &str, body: &str, indent: usize) -> String {
    let Some((first, continuation)) = header.split_once('\n') else {
        return format!("\n## {header}\n{body}");
    };
    let blank = |line: &str| line.chars().all(crate::parse::python_whitespace);
    let lines = continuation.split('\n');
    let margin = lines
        .clone()
        .filter(|line| !blank(line))
        .map(|line| line.len() - line.trim_start_matches(' ').len())
        .fold(indent, usize::min);
    let padding = " ".repeat(indent - margin);
    let mut result = format!("\n{padding}## {first}");
    for line in lines {
        result.push('\n');
        if !blank(line) {
            result.push_str(&line[margin..]);
        }
    }
    for line in body.split('\n') {
        result.push('\n');
        if !line.is_empty() {
            result.push_str(&padding);
            result.push_str(line);
        }
    }
    result
}

pub fn summary(cases: &[TestCase], files: &[FileStats], base: &str, force: bool) -> Result<String> {
    let old = compute_stats(cases, Side::Old);
    let new = compute_stats(cases, Side::New);
    if new.true_positives == 0 {
        return Err(assertion::missing_true_positives(cases));
    }
    let old_passing = files.iter().filter(|file| file.old.passes()).count() as i64;
    let new_passing = files.iter().filter(|file| file.new.passes()).count() as i64;
    let passing_change = new_passing - old_passing;
    let precision_change = new.precision() - old.precision();
    let recall_change = new.recall() - old.recall();
    let true_pos_change = new.true_positives - old.true_positives;
    let false_pos_change = new.false_positives - old.false_positives;
    let false_neg_change = new.false_negatives - old.false_negatives;
    let total_change = new.total_diagnostics - old.total_diagnostics;
    let paragraph = format!(
        "The percentage of diagnostics emitted that were expected errors {}. The percentage of expected errors that received a diagnostic {}. The number of fully passing files {}.",
        metric(precision_change, old.precision(), new.precision()),
        metric(recall_change, old.recall(), new.recall()),
        integer_metric(passing_change, old_passing, new_passing, files.len())
    );
    let base_header = format!("[Typing conformance results]({base})");
    if !force && cases.iter().all(|case| case.change() == Change::Unchanged) {
        return Ok(summary_template(
            &base_header,
            &format!(
                "\n### No changes detected ✅\n\n<details>\n<summary>Current numbers</summary>\n\n<br>\n{paragraph}\n\n</details>\n"
            ),
            12,
        ));
    }
    let improved = (precision_change > 0.0 && recall_change >= 0.0 && passing_change >= 0)
        || (recall_change > 0.0 && precision_change >= 0.0 && passing_change >= 0)
        || (passing_change > 0 && precision_change >= 0.0 && recall_change >= 0.0);
    let regressed = (precision_change < 0.0 && recall_change <= 0.0 && passing_change <= 0)
        || (recall_change < 0.0 && precision_change <= 0.0 && passing_change <= 0)
        || (passing_change < 0 && precision_change <= 0.0 && recall_change <= 0.0);
    let header = if improved {
        format!("{base_header} improved 🎉")
    } else if regressed {
        format!("{base_header} regressed ❌")
    } else {
        base_header
    };
    let total_files = files.len();
    let lines = vec![
        String::new(),
        paragraph,
        String::new(),
        "### Summary".into(),
        String::new(),
        "<details>".into(),
        "<summary>How are test cases classified?</summary>".into(),
        String::new(),
        "<br>".into(),
        String::new(),
        SUMMARY_NOTE.into(),
        String::new(),
        "</details>".into(),
        String::new(),
        "| Metric | Old | New | Diff | Outcome |".into(),
        "|--------|-----|-----|------|---------|".into(),
        format!(
            "| True Positives  | {} | {} | {true_pos_change:+} | {} |",
            old.true_positives,
            new.true_positives,
            outcome(true_pos_change as f64, true, false)
        ),
        format!(
            "| False Positives | {} | {} | {false_pos_change:+} | {} |",
            old.false_positives,
            new.false_positives,
            outcome(false_pos_change as f64, false, false)
        ),
        format!(
            "| False Negatives | {} | {} | {false_neg_change:+} | {} |",
            old.false_negatives,
            new.false_negatives,
            outcome(false_neg_change as f64, false, false)
        ),
        format!(
            "| Total Diagnostics | {} | {} | {total_change:+} | {} |",
            old.total_diagnostics,
            new.total_diagnostics,
            outcome(total_change as f64, true, true)
        ),
        format!(
            "| Precision | {} | {} | {} | {} |",
            percent(old.precision(), false),
            percent(new.precision(), false),
            percent(precision_change, true),
            outcome(precision_change, true, false)
        ),
        format!(
            "| Recall | {} | {} | {} | {} |",
            percent(old.recall(), false),
            percent(new.recall(), false),
            percent(recall_change, true),
            outcome(recall_change, true, false)
        ),
        format!(
            "| Passing Files | {old_passing}/{total_files} | {new_passing}/{total_files} | {passing_change:+} | {} |",
            outcome(passing_change as f64, true, false)
        ),
        String::new(),
        String::new(),
    ];
    Ok(summary_template(&header, &lines.join("\n"), 8))
}
