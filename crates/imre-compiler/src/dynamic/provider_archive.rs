//! Read the provider names from the native archive index without loading its machine code.

use std::collections::{HashMap, HashSet};
use std::path::Path;

const SEALED: &str = "imre_sealed_v2_";

/// Only a matching compiler can attest the result of the pack producer's lowering.
fn compiler_digest() -> Option<&'static blake3::Hash> {
    use std::sync::OnceLock;

    if !cfg!(target_os = "linux") {
        return None;
    }
    static COMPILER: OnceLock<Option<blake3::Hash>> = OnceLock::new();
    COMPILER
        .get_or_init(|| {
            // An installed executable can be replaced while its old process is still running.
            let file = std::fs::File::open("/proc/self/exe").ok()?;
            Some(blake3::Hasher::new().update_reader(file).ok()?.finalize())
        })
        .as_ref()
}

fn source_key(name: &str, path: &Path, source: &str) -> blake3::Hash {
    let mut hash = blake3::Hasher::new();
    for field in [
        SEALED.as_bytes(),
        std::env::consts::ARCH.as_bytes(),
        name.as_bytes(),
        path.as_os_str().as_encoded_bytes(),
        source.as_bytes(),
    ] {
        hash.update(&(field.len() as u64).to_le_bytes());
        hash.update(field);
    }
    hash.finalize()
}

pub(super) fn seal(symbol: &str, name: &str, path: &Path, source: &str) -> Option<String> {
    let compiler = compiler_digest()?;
    let key = source_key(name, path, source);
    // Only the archive index needs the stamp. The native link can discard its section.
    Some(format!(
        "@{SEALED}{key}_{compiler} = hidden constant ptr @{symbol}, section \".data.rel.ro.imre.provider\"\n"
    ))
}

#[derive(Default)]
pub(super) struct Providers {
    pub(super) symbols: HashSet<String>,
    sealed: HashMap<blake3::Hash, Option<(blake3::Hash, String)>>,
}

impl Providers {
    #[cfg(unix)]
    pub(super) fn read(path: &Path) -> Option<Self> {
        use object::read::{ReadCache, archive::ArchiveFile};

        let data = ReadCache::new(std::fs::File::open(path).ok()?);
        let archive = ArchiveFile::parse(&data).ok()?;
        if archive.is_thin() {
            return None;
        }
        let mut providers = Self::default();
        let mut members = HashMap::new();
        let mut seals = Vec::new();
        let mut duplicate_symbols = HashSet::new();
        for symbol in archive.symbols().ok()?? {
            let symbol = symbol.ok()?;
            let Ok(name) = std::str::from_utf8(symbol.name()) else {
                continue;
            };
            if let Some(digest) = name.strip_prefix("imre_provider_") {
                if !providers.symbols.insert(name.to_owned()) {
                    duplicate_symbols.insert(name);
                }
                if blake3::Hash::from_hex(digest).is_err() {
                    continue;
                }
                agree(&mut members, symbol.offset().0, Some(name));
            } else if let Some((key, compiler)) =
                name.strip_prefix(SEALED).and_then(|s| s.split_once('_'))
                && let (Ok(key), Ok(compiler)) = (
                    blake3::Hash::from_hex(key),
                    blake3::Hash::from_hex(compiler),
                )
            {
                seals.push((key, compiler, symbol.offset().0));
            }
        }
        for (key, compiler, member) in seals {
            let name = members
                .get(&member)
                .copied()
                .flatten()
                .filter(|name| !duplicate_symbols.contains(name));
            agree(
                &mut providers.sealed,
                key,
                name.map(|name| (compiler, name.to_owned())),
            );
        }
        Some(providers)
    }

    #[cfg(not(unix))]
    pub(super) fn read(_: &Path) -> Option<Self> {
        None
    }

    pub(super) fn sealed(&self, name: &str, path: &Path, source: &str) -> Option<&str> {
        if self.sealed.is_empty() {
            return None;
        }
        let (producer, symbol) = self.sealed.get(&source_key(name, path, source))?.as_ref()?;
        (compiler_digest()? == producer).then_some(symbol.as_str())
    }
}

/// Ambiguous archive entries cannot attest a single canonical provider.
#[cfg(unix)]
fn agree<K: std::hash::Hash + Eq, V: Eq>(
    entries: &mut HashMap<K, Option<V>>,
    key: K,
    value: Option<V>,
) {
    use std::collections::hash_map::Entry;

    match entries.entry(key) {
        Entry::Occupied(mut entry) if entry.get() != &value => {
            entry.insert(None);
        }
        Entry::Occupied(_) => {}
        Entry::Vacant(entry) => {
            entry.insert(value);
        }
    }
}
