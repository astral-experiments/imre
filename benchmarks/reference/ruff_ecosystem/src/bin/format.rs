//! A native Rust rendering baseline for the Git patches produced by Ruff's format checks.

use regex::Regex;
use std::borrow::Cow;
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write as _};
use std::path::Path;

#[derive(Default)]
struct Counts {
    added: usize,
    removed: usize,
    modified: usize,
}

#[derive(Default)]
struct File<'a> {
    source: &'a str,
    target: &'a str,
    hunks: usize,
    empty_side: bool,
}

impl File<'_> {
    fn modified(&self) -> bool {
        !self.source.is_empty()
            && self.source != "/dev/null"
            && self.target != "/dev/null"
            && !(self.hunks == 1 && self.empty_side)
    }

    fn path(&self) -> Cow<'_, str> {
        let path = if self.target == "/dev/null" {
            self.source
        } else {
            self.target
        };
        let quoted = path.strip_prefix('"').and_then(|p| p.strip_suffix('"'));
        let raw = quoted.unwrap_or(path);
        let stripped = raw
            .strip_prefix("a/")
            .or_else(|| raw.strip_prefix("b/"))
            .unwrap_or(raw);
        if quoted.is_some() {
            format!("\"{stripped}\"").into()
        } else {
            stripped.into()
        }
    }
}

fn span(field: &str) -> (usize, usize) {
    let (start, len) = field[1..].split_once(',').unwrap_or((&field[1..], "1"));
    (start.parse().unwrap(), len.parse().unwrap())
}

fn parse_patch(raw: &str, unusual_break: &Regex, full: &str, revision: &str) -> (Counts, String) {
    // git uses LF. Ruff decodes stdout and calls Python splitlines() before unidiff.
    let normalized = unusual_break.replace_all(raw, "\n");
    let lines: Vec<_> = normalized.lines().collect();
    let mut counts = Counts::default();
    for line in &lines {
        counts.added += usize::from(line.starts_with('+') && !line.starts_with("+++"));
        counts.removed += usize::from(line.starts_with('-') && !line.starts_with("---"));
    }
    let mut file = File::default();
    let mut rendered = String::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        if let Some(pair) = line.strip_prefix("diff --git ") {
            counts.modified += usize::from(file.modified());
            let (a, b) = pair
                .rsplit_once(" b/")
                .map(|(a, _)| (a, &pair[a.len() + 1..]))
                .or_else(|| {
                    pair.rsplit_once(" \"b/")
                        .map(|(a, _)| (a, &pair[a.len() + 1..]))
                })
                .unwrap_or_else(|| pair.rsplit_once(' ').unwrap());
            file = File {
                source: a,
                target: b,
                ..File::default()
            };
        } else if let Some(name) = line.strip_prefix("--- ") {
            let source = name.split('\t').next().unwrap();
            if !file.source.is_empty() && file.source != source {
                counts.modified += usize::from(file.modified());
                file = File::default();
            }
            file.source = source;
        } else if let Some(name) = line.strip_prefix("+++ ") {
            file.target = name.split('\t').next().unwrap();
        } else if line.starts_with("new file mode ") {
            file.source = "/dev/null";
        } else if line.starts_with("deleted file mode ") {
            file.target = "/dev/null";
        } else if let Some(hunk) = line.strip_prefix("@@ ") {
            let mut fields = hunk.split(' ');
            let (src, mut old) = span(fields.next().unwrap());
            let (tgt, mut new) = span(fields.next().unwrap());
            file.hunks += 1;
            file.empty_side |= (src == 0 && old == 0) || (tgt == 0 && new == 0);
            let path = file.path();
            write!(
                rendered,
                "<a href='https://github.com/{full}/blob/{revision}/{path}"
            )
            .unwrap();
            if src > 0 {
                write!(rendered, "#L{src}").unwrap();
            }
            writeln!(rendered, "'>{path}~L{src}</a>\n```diff").unwrap();
            while old != 0 || new != 0 {
                let row = lines[i];
                i += 1;
                match row.as_bytes().first() {
                    Some(b'+') => new -= 1,
                    Some(b'-') => old -= 1,
                    Some(b'\\') => {}
                    _ => {
                        old -= 1;
                        new -= 1;
                    }
                }
                writeln!(rendered, "{}", if row.is_empty() { " " } else { row }).unwrap();
            }
            while i < lines.len()
                && (lines[i].starts_with("\\ No newline at end of file")
                    || lines[i].is_empty() && i + 1 < lines.len())
            {
                writeln!(rendered, "{}", lines[i]).unwrap();
                i += 1;
            }
            rendered.push_str("```\n");
        }
    }
    counts.modified += usize::from(file.modified());
    rendered.pop();
    (counts, rendered)
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn section(out: &mut String, name: &str, title: &str, content: &str, preview: bool, exclude: &str) {
    writeln!(
        out,
        "<details><summary><a href=\"https://github.com/{name}\">{name}</a> ({title})</summary>"
    )
    .unwrap();
    if preview || !exclude.is_empty() {
        write!(
            out,
            "<p>\n<pre>ruff format --{}preview",
            if preview { "" } else { "no-" }
        )
        .unwrap();
        if !exclude.is_empty() {
            write!(out, " --exclude {exclude}").unwrap();
        }
        out.push_str("</pre>\n</p>\n");
    }
    writeln!(out, "<p>\n\n{content}\n\n</p>\n</details>").unwrap();
}

fn replay(dir: &Path, copies: usize) -> String {
    let breaks = Regex::new(r"\r\n|[\r\x0b\x0c\x1c-\x1e\x{85}\x{2028}\x{2029}]").unwrap();
    let manifest = fs::read_to_string(dir.join("projects.tsv")).unwrap();
    let mut total = Counts::default();
    let (mut changed, mut unchanged, mut errors) = (0, 0, 0);
    let (mut good, mut bad) = (String::new(), String::new());
    for copy in 0..copies {
        for row in manifest.lines() {
            let fields: [&str; 7] = row.split('\t').collect::<Vec<_>>().try_into().unwrap();
            let [owner, name, revision, preview, exclude, kind, filename] = fields;
            let mut full = format!("{owner}/{name}");
            if copy > 0 {
                write!(full, "-copy-{copy}").unwrap();
            }
            let raw = fs::read_to_string(dir.join(filename)).unwrap();
            if kind == "error" {
                errors += 1;
                section(
                    &mut bad,
                    &full,
                    "error",
                    &format!("```\n{}\n```", raw.trim()),
                    preview == "1",
                    exclude,
                );
                continue;
            }
            let (counts, rendered) = parse_patch(&raw, &breaks, &full, revision);
            total.added += counts.added;
            total.removed += counts.removed;
            total.modified += counts.modified;
            if counts.added == 0 && counts.removed == 0 {
                unchanged += 1;
                continue;
            }
            changed += 1;
            let title = format!(
                "+{} -{} lines across {} file{}",
                counts.added,
                counts.removed,
                counts.modified,
                plural(counts.modified)
            );
            section(&mut good, &full, &title, &rendered, preview == "1", exclude);
        }
    }

    if total.added == 0 && total.removed == 0 && errors == 0 {
        return "✅ ecosystem check detected no format changes.\n".to_owned();
    }
    // Ruff's formatter selects this headline on zero additions, even with removals.
    let mut out = if total.added == 0 {
        format!(
            "ℹ️ ecosystem check **encountered format errors**. (no format changes; {errors} project error{})\n",
            plural(errors)
        )
    } else {
        let mut summary = format!(
            "ℹ️ ecosystem check **detected format changes**. (+{} -{} lines in {} file{} in {changed} projects",
            total.added,
            total.removed,
            total.modified,
            plural(total.modified)
        );
        if errors > 0 {
            write!(summary, "; {errors} project error{}", plural(errors)).unwrap();
        }
        if unchanged > 0 {
            write!(
                summary,
                "; {unchanged} project{} unchanged",
                plural(unchanged)
            )
            .unwrap();
        }
        summary.push_str(")\n");
        summary
    };
    out.push('\n');
    out.push_str(&good);
    out.push_str(&bad);
    out
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().expect("capture directory");
    let copies = args.next().expect("copies").parse().unwrap();
    io::stdout()
        .lock()
        .write_all(replay(Path::new(&dir), copies).as_bytes())
        .unwrap();
}
