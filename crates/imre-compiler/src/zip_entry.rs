//! Resolve ZIP member snapshots for native lowering and Python's ZIP loader.

use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::{Path, PathBuf};

use zip::ZipArchive;

use crate::watch::FileSystem;

pub(crate) struct Member {
    pub(crate) path: PathBuf,
    pub(crate) source: String,
    pub(crate) package: bool,
    pub(crate) digest: [u8; 32],
}

/// Locate the archive for a Python CLI operand, including a path inside an archive.
fn archive_path<'a>(
    path: &'a Path,
    filesystem: &mut FileSystem<'_>,
    mut inputs: Option<&mut crate::SourceInputs>,
) -> Option<&'a Path> {
    for part in path.components().as_path().ancestors() {
        let is_file = filesystem.is_file(part);
        if let Some(inputs) = inputs.as_deref_mut() {
            inputs.file_checks.insert(part.to_owned(), is_file);
        }
        if is_file || filesystem.exists(part) {
            return is_file.then_some(part);
        }
    }
    None
}

/// Test the archive directory itself, so executable stubs and arbitrary suffixes work.
pub fn is_zip_path(path: &Path) -> bool {
    archive_path(path, &mut FileSystem::default(), None)
        .and_then(|path| File::open(path).ok())
        .is_some_and(|file| ZipArchive::new(BufReader::new(file)).is_ok())
}

/// Find an imported module in the SDK archive's existing build-time member index.
pub(crate) fn packaged_member(name: &str) -> Option<&'static str> {
    imre_ast::providers::module_member(name, PACKAGED_MEMBERS.lines())
}

/// Archive members that also travel with a standalone executable when imported.
pub(crate) fn standalone_member(name: &str) -> Option<&'static str> {
    let members = include_str!("../stdlib/runtime-only-sources.txt")
        .lines()
        .filter_map(|entry| entry.strip_prefix("standalone "));
    imre_ast::providers::module_member(name, members)
}

/// Exact installed standard library bytes shared by the native compiler and CLI.
pub const fn packaged_stdlib_zip() -> &'static [u8] {
    const LENGTH: usize = include_bytes!(concat!(env!("OUT_DIR"), "/imre-stdlib.zip")).len();
    static BYTES: [u8; LENGTH] = *include_bytes!(concat!(env!("OUT_DIR"), "/imre-stdlib.zip"));
    &BYTES
}

/// Validate the bounds and UTF-8 of each indexed ZIP payload at compile time.
pub(crate) const fn stored_source(start: usize, len: usize) -> Option<&'static str> {
    let (_, tail) = packaged_stdlib_zip().split_at(start);
    let (bytes, _) = tail.split_at(len);
    match std::str::from_utf8(bytes) {
        Ok(source) => Some(source),
        Err(_) => None,
    }
}

// Both the ordinary lookup and the fixed-source macro borrow the indexed ZIP payload.
// A fixed registration must also match the physical source root used by the ZIP writer.
macro_rules! packaged_sources {
    ($(($member:tt [$($package:tt)?] $start:literal $length:literal))*) => {
        pub(crate) const PACKAGED_MEMBERS: &str = {
            let names = concat!($($member, "\n",)*);
            names.split_at(names.len().saturating_sub(1)).0
        };

        pub(crate) fn installed_source(member: &str) -> Result<&'static str, String> {
            match member {
                $($member => const { stored_source($start, $length) },)*
                _ => None,
            }.ok_or_else(|| format!("cannot read installed source {member}"))
        }

        macro_rules! packaged_fixed {
            $((($member $($package)?), $fallback:expr) => {
                Some(($member, const {
                    $crate::zip_entry::stored_source($start, $length).expect("required bundled source")
                }))
            };)*
            ($origin:tt, $fallback:expr) => { $fallback };
        }
        pub(crate) use packaged_fixed;
    };
}
include!(concat!(env!("OUT_DIR"), "/imre-stdlib-sources.rs"));

pub(crate) struct ZipSources {
    path: PathBuf,
    archive: ZipArchive<BufReader<File>>,
    prefix: String,
}

/// Return the dotted import name for a Python member relative to its search root.
pub(crate) fn python_module_name(relative: &str) -> Option<String> {
    let stem = relative
        .strip_suffix("/__init__.py")
        .or_else(|| relative.strip_suffix(".py"))?;
    if stem.contains('.') {
        return None;
    }
    let name = stem.replace('/', ".");
    crate::modules::valid_module_operand(&name).then_some(name)
}

impl ZipSources {
    pub(crate) fn open(
        path: &Path,
        inputs: &mut crate::SourceInputs,
        filesystem: &mut FileSystem<'_>,
    ) -> Result<Self, String> {
        Self::discover(path, inputs, filesystem)?
            .ok_or_else(|| format!("{}: no readable ZIP archive", path.display()))
    }

    pub(crate) fn discover(
        path: &Path,
        inputs: &mut crate::SourceInputs,
        filesystem: &mut FileSystem<'_>,
    ) -> Result<Option<Self>, String> {
        let Some(archive) = archive_path(path, filesystem, Some(inputs)) else {
            return Ok(None);
        };
        let (Ok(identity), Ok((mut file, digest))) = (
            filesystem.canonicalize(archive),
            filesystem.open_hashed(archive),
        ) else {
            // A sys.path entry that the finder cannot open supplies no modules.
            // Revisit this lookup before an executable cache hit if the file becomes readable.
            inputs.missing.push(archive.to_owned());
            return Ok(None);
        };
        file.rewind().map_err(|e| e.to_string())?;
        let input = (archive.to_owned(), identity, digest.to_hex().to_string());
        if let Some(previous) = inputs.files.iter().find(|(path, _, _)| path == archive) {
            if previous != &input {
                return Err(format!(
                    "ZIP archive changed during compilation: {}",
                    archive.display()
                ));
            }
        } else {
            // Multiple declared search directories can belong to the same archive file.
            inputs.files.push(input);
        }
        let Ok(reader) = ZipArchive::new(BufReader::new(file)) else {
            return Ok(None);
        };
        let prefix = path
            .strip_prefix(archive)
            .map_err(|e| e.to_string())?
            .to_str()
            .ok_or("ZIP entry path is not UTF-8")?;
        Ok(Some(Self {
            path: archive.to_owned(),
            archive: reader,
            prefix: prefix.to_owned(),
        }))
    }

    fn stem(&self, name: &str) -> String {
        let stem = name.replace('.', "/");
        if self.prefix.is_empty() {
            stem
        } else {
            format!("{}/{stem}", self.prefix)
        }
    }

    /// Enumerate only Python module spellings exposed under this declared ZIP search root.
    pub(crate) fn modules(&self) -> impl Iterator<Item = String> {
        let prefix = self.prefix.as_str();
        self.archive.file_names().filter_map(move |member| {
            let relative = if prefix.is_empty() {
                member
            } else {
                member.strip_prefix(prefix)?.strip_prefix('/')?
            };
            let name = python_module_name(relative)?;
            // A resource directory without a package or a ZIP directory entry is not a finder path.
            let parents_are_importable = name.match_indices('.').all(|(end, _)| {
                let parent = &name[..end];
                self.package(parent) == Some(true) || self.contains_namespace(parent)
            });
            parents_are_importable.then_some(name)
        })
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

    /// Retain the origin and raw fingerprint used by zipimport.get_code.
    pub(crate) fn module(&mut self, name: &str) -> Option<Member> {
        use sha2::{Digest, Sha256};
        let (member, package) = self.module_member(name)?;
        if member.ends_with(".pyc") {
            return None;
        }
        let path = self.path.join(&member);
        let mut bytes = Vec::new();
        self.archive
            .by_name(&member)
            .ok()?
            .read_to_end(&mut bytes)
            .ok()?;
        let digest = Sha256::digest(&bytes).into();
        let source =
            imre_ast::source_encoding::decode_compile_source(bytes, |_, error| error.to_string())
                .ok()?;
        Some(Member {
            path,
            source,
            package,
            digest,
        })
    }
}
