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
    for name in include_str!("stdlib/installed-sources.txt").lines() {
        let path = Path::new("stdlib").join(name);
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
