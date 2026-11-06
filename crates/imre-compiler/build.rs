//! Package the Python sources that the boxed importer loads on demand.
use std::fs::File;
use std::io;
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
    // Only installed-sources.txt participates in checked file-entry admission.
    let members: Vec<_> = include_str!("stdlib/installed-sources.txt")
        .lines()
        .chain(include_str!("stdlib/runtime-only-sources.txt").lines())
        .collect();
    std::fs::write(out.join("imre-stdlib-members.txt"), members.join("\n"))?;
    for name in members {
        let path = match name {
            "shlex.py" => {
                PathBuf::from("../../benchmarks/cpython_stdlib/full/shlex/upstream/Lib/shlex.py")
            }
            _ => Path::new("stdlib").join(name),
        };
        println!("cargo:rerun-if-changed={}", path.display());
        archive.start_file(name, options)?;
        io::copy(&mut File::open(path)?, &mut archive)?;
    }
    archive.set_comment(concat!(
        include_str!("../../licenses/CPython.txt"),
        "\n",
        include_str!("../../licenses/ElementTree.txt")
    ))?;
    archive.finish()?;
    Ok(())
}
