use crate::model::{Change, Classification, Side, Statistics, compute_stats, group_diagnostics};
use crate::{Error, Result, parse};
use std::hint::black_box;
use std::io::{self, Write};

fn label(classification: Classification) -> &'static str {
    match classification {
        Classification::TruePositive => "true_positive",
        Classification::FalsePositive => "false_positive",
        Classification::TrueNegative => "true_negative",
        Classification::FalseNegative => "false_negative",
    }
}

fn score(s: &Statistics) -> i64 {
    s.true_positives + 3 * s.false_positives + 5 * s.false_negatives + 7 * s.total_diagnostics
}

pub(crate) fn run(args: &[String]) -> Result<()> {
    let [manifest, old, new, rounds, mode] = args else {
        return Err(Error::value(
            "expected --classify MANIFEST OLD NEW ROUNDS checksum|dump",
        ));
    };
    let inputs = parse::read_inputs(manifest, old, new)?;
    let expected = parse::collect_expected(&inputs.sources)?;
    let old = parse::collect_diagnostics(&inputs.old, &inputs.paths)?;
    let new = parse::collect_diagnostics(&inputs.new, &inputs.paths)?;
    let mut cases = group_diagnostics(old, new, expected);
    cases.sort_by(|a, b| a.key.cmp(&b.key));
    let rounds: usize = rounds.parse().map_err(|_| Error::value("invalid rounds"))?;
    let dump = mode == "dump";
    let mut out = io::stdout().lock();
    if dump {
        for case in &cases {
            let change = match case.change() {
                Change::Added => "added",
                Change::Removed => "removed",
                Change::Changed => "changed",
                Change::Unchanged => "unchanged",
            };
            writeln!(
                out,
                "{}\t{}\t{}\t{change}",
                case.key,
                label(case.classify(Side::Old)),
                label(case.classify(Side::New))
            )
            .map_err(Error::io)?;
        }
    }
    let mut checksum = 0;
    for _ in 0..rounds {
        let old = compute_stats(black_box(&cases), Side::Old);
        let new = compute_stats(black_box(&cases), Side::New);
        checksum += score(black_box(&old)) + 11 * score(black_box(&new));
        if dump {
            for stats in [old, new] {
                writeln!(
                    out,
                    "{} {} {} {}",
                    stats.true_positives,
                    stats.false_positives,
                    stats.false_negatives,
                    stats.total_diagnostics
                )
                .map_err(Error::io)?;
            }
        }
    }
    writeln!(out, "{} {checksum}", cases.len()).map_err(Error::io)
}
