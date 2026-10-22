//! Shared deterministic child for process/capture benchmarks, not a report engine.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

/// Both aliases execute these exact bytes. Their names choose the retained old
/// or new capture, just as two pinned ty executables would produce two reports.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<_> = std::env::args().collect();
    if arguments.len() != 3 {
        return Err("expected OLD_JSON NEW_JSON".into());
    }
    if std::env::var("TY_MEMORY_REPORT")?.as_str() != "json"
        || std::env::var("TY_MAX_PARALLELISM")?.as_str() != "1"
    {
        return Err("memory-report environment was not configured".into());
    }
    let name = Path::new(&arguments[0])
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("producer executable has no UTF-8 filename")?;
    let index = if name.ends_with("-old") {
        1
    } else if name.ends_with("-new") {
        2
    } else {
        return Err("invoke the producer through its -old or -new alias".into());
    };
    let report = fs::read(&arguments[index])?;
    let diagnostics = fs::read("stderr.bin")?;
    let mut stdout = io::stdout().lock();
    stdout.write_all(&report)?;
    stdout.flush()?;
    let mut stderr = io::stderr().lock();
    stderr.write_all(&diagnostics)?;
    stderr.flush()?;
    Ok(())
}
