use std::fs;
use std::io::{self, Write};

fn main() -> io::Result<()> {
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    for path in fs::read_to_string(std::env::args_os().nth(1).expect("missing manifest"))?.lines() {
        let text = fs::read_to_string(path)?;
        for (i, line) in text.lines().enumerate() {
            if line.contains("TODO") {
                writeln!(stdout, "{path}:{}", i + 1)?;
            }
        }
    }
    stdout.flush()
}
