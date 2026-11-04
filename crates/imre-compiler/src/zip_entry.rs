//! Discover imports of a ZIP application while leaving execution to zipimport and runpy.

use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek};
use std::path::{Path, PathBuf};

use zip::ZipArchive;

/// Locate the archive for a Python CLI operand, including a path inside an archive.
fn archive_path(path: &Path) -> Option<&Path> {
    let path = path.components().as_path();
    let archive = path.ancestors().find(|part| part.exists())?;
    archive.is_file().then_some(archive)
}

/// Test the archive directory itself, so executable stubs and arbitrary suffixes work.
pub fn is_zip_path(path: &Path) -> bool {
    archive_path(path)
        .and_then(|path| File::open(path).ok())
        .is_some_and(|file| ZipArchive::new(BufReader::new(file)).is_ok())
}

/// Match parsed import names against the same member list used to package the stdlib.
pub(crate) fn installed_member(name: &str) -> Option<&'static str> {
    include_str!("../stdlib/installed-sources.txt")
        .lines()
        .find(|file| {
            file.strip_suffix("/__init__.py")
                .or_else(|| file.strip_suffix(".py"))
                .is_some_and(|module| module.split('/').eq(name.split('.')))
        })
}

fn read_source<R: Read + Seek>(archive: &mut ZipArchive<R>, member: &str) -> Option<String> {
    std::io::read_to_string(archive.by_name(member).ok()?).ok()
}

/// Read a selected member only after checking earlier application sources.
pub(crate) fn installed_source(member: &str) -> Result<String, String> {
    const RESOURCE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/imre-stdlib.zip"));
    let mut archive = ZipArchive::new(Cursor::new(RESOURCE)).map_err(|e| e.to_string())?;
    read_source(&mut archive, member)
        .ok_or_else(|| format!("cannot read installed source {member}"))
}

pub(crate) struct ZipSources {
    archive: ZipArchive<BufReader<File>>,
    prefix: String,
}

impl ZipSources {
    pub(crate) fn open(path: &Path, inputs: &mut crate::SourceInputs) -> Result<Self, String> {
        let archive =
            archive_path(path).ok_or_else(|| format!("{}: archive not found", path.display()))?;
        let mut file = File::open(archive).map_err(|e| e.to_string())?;
        let digest = blake3::Hasher::new()
            .update_reader(&mut file)
            .map_err(|e| e.to_string())?
            .finalize();
        file.rewind().map_err(|e| e.to_string())?;
        let reader = ZipArchive::new(BufReader::new(file)).map_err(|e| e.to_string())?;
        inputs.files.push((
            archive.to_owned(),
            archive.canonicalize().map_err(|e| e.to_string())?,
            digest.to_hex().to_string(),
        ));
        let prefix = path
            .strip_prefix(archive)
            .map_err(|e| e.to_string())?
            .to_str()
            .ok_or("ZIP entry path is not UTF-8")?;
        Ok(Self {
            archive: reader,
            prefix: prefix.to_owned(),
        })
    }

    fn stem(&self, name: &str) -> String {
        let stem = name.replace('.', "/");
        if self.prefix.is_empty() {
            stem
        } else {
            format!("{}/{stem}", self.prefix)
        }
    }

    /// Follow zipimport's package-before-module and bytecode-before-source search.
    fn module_member(&self, name: &str) -> Option<(String, bool)> {
        let stem = self.stem(name);
        for (suffix, package) in [
            ("/__init__.pyc", true),
            ("/__init__.py", true),
            (".pyc", false),
            (".py", false),
        ] {
            let member = format!("{stem}{suffix}");
            if self.archive.index_for_name(&member).is_some() {
                return Some((member, package));
            }
        }
        None
    }

    /// Return whether the selected ZIP member is a package, even if it has no readable source.
    pub(crate) fn package(&self, name: &str) -> Option<bool> {
        self.module_member(name).map(|(_, package)| package)
    }

    /// An explicit directory entry can supply a namespace portion when no concrete member wins.
    pub(crate) fn contains_namespace(&self, name: &str) -> bool {
        let directory = format!("{}/", self.stem(name));
        self.archive.index_for_name(&directory).is_some()
    }

    /// Only collect import hints. The launch-time loader reports source and bytecode errors.
    pub(crate) fn module(&mut self, name: &str) -> Option<(PathBuf, String, bool)> {
        let (member, package) = self.module_member(name)?;
        if member.ends_with(".pyc") {
            return None;
        }
        let source = read_source(&mut self.archive, &member)?;
        Some((member.into(), source, package))
    }
}
