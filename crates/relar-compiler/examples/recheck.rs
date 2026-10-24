//! Run `cargo run -p relar-compiler --example recheck -- path/to/main.py`.
//! Edit a source and press Enter to check another revision in the same process.

use std::io::{self, BufRead};
use std::path::PathBuf;

use relar_compiler::IncrementalCompiler;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let entry = PathBuf::from(std::env::args_os().nth(1).ok_or("expected an entry file")?);
    let mut compiler = IncrementalCompiler::default();
    let mut lines = io::stdin().lock().lines();
    loop {
        match compiler.check_file(&entry, None) {
            Ok(()) => println!("ok"),
            Err(diagnostics) => eprintln!("{diagnostics}"),
        }
        if lines.next().transpose()?.is_none() {
            return Ok(());
        }
    }
}
