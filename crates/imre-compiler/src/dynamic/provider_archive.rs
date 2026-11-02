//! Read the provider names from the native archive index without loading its machine code.

use std::collections::HashSet;
use std::path::Path;

#[cfg(unix)]
pub(super) fn symbols(path: &Path) -> Option<HashSet<String>> {
    use object::read::{ReadCache, archive::ArchiveFile};

    let data = ReadCache::new(std::fs::File::open(path).ok()?);
    let archive = ArchiveFile::parse(&data).ok()?;
    if archive.is_thin() {
        return None;
    }
    let mut names = HashSet::new();
    for symbol in archive.symbols().ok()?? {
        let symbol = symbol.ok()?;
        if symbol.name().starts_with(b"imre_provider_") {
            names.insert(std::str::from_utf8(symbol.name()).ok()?.to_owned());
        }
    }
    Some(names)
}

#[cfg(not(unix))]
pub(super) fn symbols(_: &Path) -> Option<HashSet<String>> {
    None
}
