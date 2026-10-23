//! Observe the reporter's actual grouping, equivalence and per-file statistics.

use crate::model::{
    FileStats, Statistics, TestCase, diagnostics_are_equivalent, group_diagnostics,
};
use crate::{Error, Result, collect_file_stats, parse};
use std::hint::black_box;
use std::io::{self, Write};

fn escaped(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn score(s: &Statistics) -> i64 {
    s.true_positives + 3 * s.false_positives + 5 * s.false_negatives + 7 * s.total_diagnostics
}

fn stats_text(s: &Statistics) -> String {
    format!(
        "{}\t{}\t{}\t{}",
        s.true_positives, s.false_positives, s.false_negatives, s.total_diagnostics
    )
}

fn dump(cases: &[TestCase], files: &[FileStats]) -> io::Result<()> {
    let mut out = io::stdout().lock();
    for case in cases {
        writeln!(
            out,
            "G\t{}\t{}",
            escaped(&case.key),
            u8::from(diagnostics_are_equivalent(&case.old, &case.new))
        )?;
        for (side, items) in [("old", &case.old), ("new", &case.new)] {
            for d in items {
                let b = &d.positions.begin;
                let e = &d.positions._end;
                writeln!(
                    out,
                    "D\t{side}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    escaped(&d.check_name),
                    escaped(&d.description),
                    escaped(&d.severity),
                    escaped(&d.path),
                    b.line,
                    b.column,
                    e.line,
                    e.column
                )?;
            }
        }
        for e in &case.expected {
            let tag = e
                .tag
                .as_deref()
                .map(|t| format!("+{}", escaped(t)))
                .unwrap_or_else(|| "-".to_owned());
            writeln!(
                out,
                "E\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                escaped(&e._description),
                escaped(&e.path),
                e.line,
                e._begin_column,
                e.line,
                e._end_column,
                u8::from(e.optional),
                tag,
                u8::from(e.multi)
            )?;
        }
    }
    for f in files {
        writeln!(
            out,
            "F\t{}\t{}\t{}",
            escaped(&f.path),
            stats_text(&f.old),
            stats_text(&f.new)
        )?;
    }
    Ok(())
}

pub(crate) fn run(args: &[String]) -> Result<()> {
    let [mode, manifest, old, new, rounds] = args else {
        return Err(Error::value("expected MODE MANIFEST OLD NEW ROUNDS"));
    };
    let rounds: usize = rounds.parse().map_err(|_| Error::value("invalid rounds"))?;
    let inputs = parse::read_inputs(manifest, old, new)?;
    let mut old = parse::collect_diagnostics(&inputs.old, &inputs.paths)?;
    let new = parse::collect_diagnostics(&inputs.new, &inputs.paths)?;
    let expected = parse::collect_expected(&inputs.sources)?;
    let mut checksum = 0;
    if mode == "aliases" {
        let mut cases = group_diagnostics(&old, &new, &expected);
        let matches = old.first().map_or(0, |first| {
            cases
                .iter()
                .flat_map(|case| &case.old)
                .filter(|d| std::ptr::eq(**d, first))
                .count()
        });
        let empty = cases.iter().filter(|c| c.old.is_empty()).count();
        if let (Some(d), Some(case)) = (old.first(), cases.iter_mut().find(|c| c.old.is_empty())) {
            case.old.push(d);
        }
        let remaining = cases.iter().filter(|c| c.old.is_empty()).count();
        println!("{matches} {empty} {remaining}");
        return Ok(());
    }
    if mode == "group" {
        for _ in 0..rounds {
            if old.len() > 1 {
                let last = old.len() - 1;
                old.swap(0, last);
            }
            let groups = group_diagnostics(black_box(&old), black_box(&new), black_box(&expected));
            for (i, c) in black_box(&groups).iter().enumerate() {
                checksum +=
                    ((i + 1) * (1 + c.old.len() + 3 * c.new.len() + 5 * c.expected.len())) as i64;
            }
        }
    } else {
        let mut cases = group_diagnostics(old, new, expected);
        if mode == "dump" {
            return dump(&cases, &collect_file_stats(&cases, &inputs)?).map_err(Error::io);
        }
        for _ in 0..rounds {
            if cases.len() > 1 {
                let last = cases.len() - 1;
                cases.swap(0, last);
            }
            if mode == "equivalent" {
                for (i, case) in black_box(&cases).iter().enumerate() {
                    if diagnostics_are_equivalent(&case.old, &case.new) {
                        checksum += i as i64 + 1;
                    }
                }
            } else if mode == "files" {
                for (i, f) in collect_file_stats(black_box(&cases), &inputs)?
                    .iter()
                    .enumerate()
                {
                    checksum += (i as i64 + 1) * (1 + score(&f.old) + 11 * score(&f.new));
                }
            } else {
                return Err(Error::value("unknown analysis mode"));
            }
        }
    }
    println!("{checksum}");
    Ok(())
}
