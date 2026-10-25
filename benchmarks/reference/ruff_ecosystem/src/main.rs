mod alignment;

use indexmap::IndexMap;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fs;
use std::path::Path;

type Counts = [IndexMap<String, usize>; 4];
const OPTIONS: &str = "<pre>ruff check --no-cache --exit-zero --no-fix --output-format concise --no-preview --select ALL</pre>";

struct Patterns {
    lines: Regex,
    summary: Regex,
    diagnostic: Regex,
    link: Regex,
}

impl Patterns {
    fn new() -> Self {
        Self {
            lines: Regex::new(r"\r\n|[\n\r\x0b\x0c\x1c-\x1e\x{85}\x{2028}\x{2029}]").unwrap(),
            summary: Regex::new(r"^(?:Found \d+ error.*|.* fixable with .*$)").unwrap(),
            diagnostic: Regex::new(r"^(.*?): ([A-Z]{1,5}[0-9]{3,4}|[a-z0-9-]+):?( \[\*\])? (.*)")
                .unwrap(),
            link: Regex::new(r"^([+-]) (([^:]+):(\d+):\d+:) (.*)$").unwrap(),
        }
    }

    fn stream<'a>(&self, source: &'a str) -> Vec<&'a str> {
        let mut lines = Vec::new();
        let mut start = 0;
        for end in self.lines.find_iter(source) {
            let line = &source[start..end.start()];
            if !self.summary.is_match(line) {
                lines.push(line);
            }
            start = end.end();
        }
        if start < source.len() && !self.summary.is_match(&source[start..]) {
            lines.push(&source[start..]);
        }
        lines.sort_unstable();
        lines
    }
}

struct Diagnostic {
    line: String,
    rule: String,
}

struct Project {
    full_name: String,
    revision: String,
    changed: bool,
    counts: Counts,
    diagnostics: Vec<Diagnostic>,
}

fn replay_project(
    full_name: String,
    revision: &str,
    old: &str,
    new: &str,
    re: &Patterns,
) -> Project {
    let changes = alignment::changes(&re.stream(old), &re.stream(new));
    let mut rows = [HashSet::new(), HashSet::new()];
    for &(added, row) in &changes {
        rows[usize::from(added)].insert(row);
    }
    let mut project = Project {
        full_name,
        revision: revision.to_string(),
        changed: !changes.is_empty(),
        counts: Counts::default(),
        diagnostics: Vec::new(),
    };
    for (added, row) in changes {
        let Some(m) = re.diagnostic.captures(row) else {
            continue;
        };
        let (location, code, fix, message) = (&m[1], &m[2], m.get(3).is_some(), &m[4]);
        let alternate = format!(
            "{location}: {code} {}{message}",
            if fix { "" } else { "[*] " }
        );
        let category = if rows[usize::from(!added)].contains(alternate.as_str()) {
            if added == fix { 2 } else { 3 }
        } else if added {
            0
        } else {
            1
        };
        *project.counts[category].entry(code.to_owned()).or_default() += 1;
        let sign = if added { '+' } else { '-' };
        let marker = if fix { "[*] " } else { "" };
        project.diagnostics.push(Diagnostic {
            line: format!("{sign} {location}: {code} {marker}{message}"),
            rule: code.to_owned(),
        });
    }
    project
}

fn by_rule(counts: &Counts) -> IndexMap<&str, usize> {
    let mut totals = IndexMap::new();
    for category in counts {
        for (code, count) in category {
            *totals.entry(code.as_str()).or_default() += count;
        }
    }
    totals
}

fn details(lines: &mut Vec<String>, title: &str, preface: Option<&str>, body: Vec<String>) {
    lines.push(format!("<details><summary>{title}</summary>"));
    if let Some(preface) = preface {
        lines.extend(["<p>".into(), preface.into(), "</p>".into()]);
    }
    lines.extend(["<p>".into(), "".into()]);
    lines.extend(body);
    lines.extend(["".into(), "</p>".into(), "</details>".into()]);
}

fn report(projects: &[Project], re: &Patterns) -> String {
    let mut all = Counts::default();
    for project in projects {
        for (dst, src) in all.iter_mut().zip(&project.counts) {
            for (code, count) in src {
                *dst.entry(code.clone()).or_default() += count;
            }
        }
    }
    let mut rules = by_rule(&all);
    if rules.is_empty() {
        return "✅ ecosystem check detected no linter changes.".to_owned();
    }
    let changed = projects.iter().filter(|p| p.changed).count();
    let unchanged = projects.len() - changed;
    let totals = all.each_ref().map(|c| c.values().sum::<usize>());
    let [av, rv, af, rf] = totals;
    let suffix = match unchanged {
        0 => String::new(),
        1 => "; 1 project unchanged".to_string(),
        n => format!("; {n} projects unchanged"),
    };
    let mut lines = vec![
        format!(
            "ℹ️ ecosystem check **detected linter changes**. (+{av} -{rv} violations, +{af} -{rf} fixes in {changed} projects{suffix})"
        ),
        String::new(),
    ];
    let total: usize = totals.iter().sum();
    for project in projects {
        if lines.iter().map(|l| l.chars().count()).sum::<usize>() + lines.len() - 1 > 65536 / 3 {
            lines.extend(["".into(), "_... Truncated remaining completed project reports due to GitHub comment length restrictions_".into(), "".into()]);
            break;
        }
        if !project.changed {
            continue;
        }
        let per_rule = by_rule(&project.counts);
        let p: usize = per_rule.values().sum();
        let limit = ((p as f64 / total as f64 * 50.0) as usize).max(10);
        let rule_limit = (limit / per_rule.len().max(1)).max(5);
        let mut shown: HashMap<&str, usize> = HashMap::new();
        let mut display = 0;
        let mut body = vec!["<pre>".to_owned()];
        for Diagnostic { line, rule } in &project.diagnostics {
            let count = shown.entry(rule).or_default();
            if *count > rule_limit {
                continue;
            }
            let rendered = match re.link.captures(line) {
                Some(m) => {
                    let number = m[4].trim_start_matches('0');
                    let anchor = if number.is_empty() {
                        String::new()
                    } else {
                        format!("#L{number}")
                    };
                    format!(
                        "{} <a href='https://github.com/{}/blob/{}/{}{anchor}'>{}</a> {}",
                        &m[1], project.full_name, project.revision, &m[3], &m[2], &m[5]
                    )
                }
                None => line.clone(),
            };
            body.push(rendered);
            *count += 1;
            display += 1;
            if *count > rule_limit {
                body.push(format!(
                    "... {} additional changes omitted for rule {rule}",
                    per_rule[rule.as_str()] - rule_limit
                ));
            }
            if display >= limit {
                break;
            }
        }
        if p > limit {
            body.push(format!(
                "... {} additional changes omitted for project",
                p - display
            ));
        }
        body.push("</pre>".to_owned());
        let [av, rv, af, rf] = project.counts.each_ref().map(|c| c.values().sum::<usize>());
        let name = &project.full_name;
        details(
            &mut lines,
            &format!(
                "<a href=\"https://github.com/{name}\">{name}</a> (+{av} -{rv} violations, +{af} -{rf} fixes)"
            ),
            Some(OPTIONS),
            body,
        );
    }
    let mut table = vec![
        "| code | total | + violation | - violation | + fix | - fix |".into(),
        "| ---- | ------- | --------- | -------- | ----- | ---- |".into(),
    ];
    rules.sort_by(|_, a, _, b| b.cmp(a));
    for (rule, count) in &rules {
        let [av, rv, af, rf] = all.each_ref().map(|c| c.get(*rule).copied().unwrap_or(0));
        table.push(format!("| {rule} | {count} | {av} | {rv} | {af} | {rf} |"));
    }
    details(
        &mut lines,
        &format!("Changes by rule ({} rules affected)", rules.len()),
        None,
        table,
    );
    lines.join("\n")
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().collect();
    let [_, directory, copies] = args.as_slice() else {
        return Err("expected CAPTURE_DIR COPIES".into());
    };
    let directory = Path::new(directory);
    let manifest = fs::read_to_string(directory.join("projects.tsv"))?;
    let re = Patterns::new();
    let mut projects = Vec::new();
    for copy in 0..copies.parse::<usize>()? {
        for row in manifest.lines() {
            let fields: Vec<_> = row.split('\t').collect();
            let [owner, name, revision, old, new] = fields.as_slice() else {
                return Err("invalid project row".into());
            };
            let full_name = if copy == 0 {
                format!("{owner}/{name}")
            } else {
                format!("{owner}/{name}-copy-{copy}")
            };
            projects.push(replay_project(
                full_name,
                revision,
                &fs::read_to_string(directory.join(old))?,
                &fs::read_to_string(directory.join(new))?,
                &re,
            ));
        }
    }
    print!("{}", report(&projects, &re));
    Ok(())
}
