use std::hint::black_box;
use std::io::Write;

use super::{annotation, read_text};
use crate::{Error, Result};

/// Observe all captures from the same scanner used by the independent Rust reporter.
pub(crate) fn run(args: &[String]) -> Result<()> {
    let [path, rounds, mode] = args else {
        return Err(Error::value("expected --scan CORPUS ROUNDS checksum|dump"));
    };
    let corpus = read_text(path)?;
    let lines: Vec<_> = corpus.split('\0').collect();
    let rounds: usize = rounds.parse().map_err(|_| Error::value("invalid rounds"))?;
    let dump = mode == "dump";
    let mut output = std::io::stdout().lock();
    let mut matches = 0_usize;
    let mut checksum = 0_usize;
    for _ in 0..rounds {
        for (number, &line) in black_box(&lines).iter().enumerate() {
            let Some(found) = annotation(line) else {
                continue;
            };
            matches += 1;
            let tag = found.tag.unwrap_or_default();
            let tag_len = tag.chars().count();
            let optional = usize::from(found.optional);
            let multi = usize::from(found.multi);
            checksum += 1
                + found.start
                + 3 * found.end
                + 5 * found.description.chars().count()
                + 7 * tag_len
                + 11 * optional
                + 13 * multi;
            if dump {
                writeln!(
                    output,
                    "{number}:{}:{}:{optional}:{multi}:{tag_len}:{tag}{}",
                    found.start, found.end, found.description
                )
                .map_err(Error::io)?;
            }
        }
    }
    writeln!(output, "{matches} {checksum}").map_err(Error::io)
}
