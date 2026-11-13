//! Package the Python sources that the boxed importer loads on demand.
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipWriter};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"));
    let mut archive = ZipWriter::new(File::create(out.join("imre-stdlib.zip"))?);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .last_modified_time(DateTime::default())
        .unix_permissions(0o644);
    println!("cargo:rerun-if-changed=stdlib/installed-sources.txt");
    println!("cargo:rerun-if-changed=stdlib/runtime-only-sources.txt");
    // The standalone marker affects source registration, not the path in the ZIP.
    let runtime_only = include_str!("stdlib/runtime-only-sources.txt")
        .lines()
        .map(|entry| entry.strip_prefix("standalone ").unwrap_or(entry));
    let members: Vec<_> = include_str!("stdlib/installed-sources.txt")
        .lines()
        .chain(runtime_only)
        .collect();
    let mut origins = Vec::with_capacity(members.len());
    for name in members {
        let package = match name {
            "http/cookies.py" => Some("http"),
            "shlex.py" => Some("shlex"),
            _ => None,
        };
        let root = match package {
            Some(package) => Path::new("../../benchmarks/cpython_stdlib/full")
                .join(package)
                .join("upstream/Lib"),
            None => PathBuf::from("stdlib"),
        };
        origins.push(package);
        let path = root.join(name);
        println!("cargo:rerun-if-changed={}", path.display());
        archive.start_file(name, options)?;
        io::copy(&mut File::open(path)?, &mut archive)?;
    }
    archive.set_comment(concat!(
        include_str!("../../licenses/CPython.txt"),
        "\n",
        include_str!("../../licenses/ElementTree.txt")
    ))?;
    // Use the writer's own offsets for the stored payloads in the embedded ZIP.
    let mut archive = archive.finish_into_readable()?;
    let mut index = File::create(out.join("imre-stdlib-sources.rs"))?;
    writeln!(index, "packaged_sources! {{")?;
    for (i, package) in origins.into_iter().enumerate() {
        let entry = archive.by_index_raw(i)?;
        assert_eq!(entry.compression(), CompressionMethod::Stored);
        let start = entry.data_start().expect("stored member has a data offset");
        let origin = package.map_or_else(String::new, |package| format!("{package:?}"));
        writeln!(
            index,
            "    ({:?} [{origin}] {start} {})",
            entry.name(),
            entry.size()
        )?;
    }
    writeln!(index, "}}")?;
    Ok(())
}
