//! CPython has independent packing and unpacking registries. The public listing
//! creates fresh tuples, but an unpacker's extension list remains live.
use super::*;
use crate::objects::records::{RecordDescriptor, allocate_record};
use crate::objects::words::{OwnedWord, WordKind, Words};
use std::cell::RefCell;

mod callback;
use callback::Callback;

struct Builtin(Format, &'static str, &'static [&'static str], &'static str);

// Extension selection follows unpacker insertion order, unlike the sorted public list.
const BUILTINS: [Builtin; 6] = [
    Builtin(Format::Tar, "tar", &[".tar"], "uncompressed tar file"),
    Builtin(Format::Zip, "zip", &[".zip"], "ZIP file"),
    Builtin(
        Format::Gzip,
        "gztar",
        &[".tar.gz", ".tgz"],
        "gzip'ed tar-file",
    ),
    Builtin(
        Format::Bzip,
        "bztar",
        &[".tar.bz2", ".tbz2"],
        "bzip2'ed tar-file",
    ),
    Builtin(Format::Xz, "xztar", &[".tar.xz", ".txz"], "xz'ed tar-file"),
    Builtin(
        Format::Zstd,
        "zstdtar",
        &[".tar.zst", ".tzst"],
        "zstd'ed tar-file",
    ),
];

#[derive(Clone)]
pub(super) enum Unpacker {
    Builtin(Format),
    Custom(Rc<Callback>),
}

#[derive(Clone)]
struct Entry {
    name: Rc<Str>,
    description: Rc<Str>,
    unpacker: Unpacker,
    suffix: &'static str,
    extensions: Rc<Object>,
}

struct Registry {
    pack: Vec<Entry>,
    unpack: Vec<Entry>,
}

impl Registry {
    fn new() -> Self {
        let unpack: Vec<_> = BUILTINS
            .iter()
            .map(|&Builtin(format, name, suffixes, description)| {
                let mut extensions = Words::new(3);
                extensions.values.extend(
                    suffixes
                        .iter()
                        .map(|&s| Rc::into_raw(Rc::new(Str::from(s))) as usize),
                );
                Entry {
                    name: Rc::new(Str::from(name)),
                    description: Rc::new(Str::from(description)),
                    unpacker: Unpacker::Builtin(format),
                    suffix: suffixes[0],
                    extensions: extensions.into_object(),
                }
            })
            .collect();
        Self {
            pack: unpack.clone(),
            unpack,
        }
    }
}

thread_local! { static REGISTRY: RefCell<Registry> = RefCell::new(Registry::new()); }

pub(super) fn archiver(name: &str) -> Option<(Format, &'static str)> {
    REGISTRY.with_borrow(|registry| {
        registry
            .pack
            .iter()
            .find(|e| e.name.as_str() == name)
            .and_then(|e| match e.unpacker {
                Unpacker::Builtin(f) => Some((f, e.suffix)),
                Unpacker::Custom(_) => None,
            })
    })
}

pub(super) fn unpacker(name: Option<&str>, filename: &str) -> Option<Unpacker> {
    REGISTRY.with_borrow(|registry| {
        registry
            .unpack
            .iter()
            .find(|e| match name {
                Some(name) => e.name.as_str() == name,
                // SAFETY: the live list is homogeneous str, and no user code runs inside this borrow.
                None => unsafe {
                    e.extensions.0.with(|s| {
                        s.sequence()
                            .strings()
                            .any(|(_, ext)| filename.ends_with(ext.as_str()))
                    })
                },
            })
            .map(|e| e.unpacker.clone())
    })
}

/// # Safety
/// The native call roots both strings, the list of strings, the extra arguments,
/// and both records. The adapter and its matching frame descriptor are static.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_shutil_register_unpack(
    name: *const Str,
    extensions: *const Object,
    function: *const usize,
    extras: usize,
    extras_kind: u32,
    description: *const Str,
    adapter: *const (),
    frame: *const RecordDescriptor,
    metadata: *const usize,
) {
    let result = REGISTRY.with_borrow_mut(|registry| -> Result<()> {
        // The last registered owner wins if an existing list was mutated to contain
        // another format's extension, as when CPython builds its temporary dictionary.
        // The caller may pass a live list previously returned by get_unpack_formats.
        // SAFETY: release the list borrow before consulting possible aliases.
        let proposed = unsafe {
            (&*extensions).0.with(|s| {
                s.sequence()
                    .strings()
                    .map(|(word, _)| acquire_rc::<Str>(word))
                    .collect::<Vec<_>>()
            })
        };
        for ext in proposed {
            let previous = registry.unpack.iter().rev().find(|entry| {
                // SAFETY: the registry owns an exact list of strings.
                unsafe {
                    entry
                        .extensions
                        .0
                        .with(|v| v.sequence().strings().any(|(_, old)| old == &*ext))
                }
            });
            if let Some(previous) = previous {
                return Err(Error::new(
                    Kind::ShutilRegistryError,
                    format!("{ext} is already registered for \"{}\"", previous.name),
                ));
            }
        }
        if adapter.is_null() {
            return Err(Error::new(
                Kind::TypeError,
                "The registered function must be a callable",
            ));
        }
        // SAFETY: the compiler passes a closed native adapter, both matching records
        // and the exact representation of the registered extra_args object.
        let entry = unsafe {
            Entry {
                name: acquire_rc(name as usize),
                description: acquire_rc(description as usize),
                extensions: acquire_rc(extensions as usize),
                suffix: "",
                unpacker: Unpacker::Custom(Rc::new(Callback::new(
                    function,
                    adapter,
                    frame,
                    metadata,
                    extras,
                    extras_kind,
                ))),
            }
        };
        if let Some(existing) = registry
            .unpack
            .iter_mut()
            .find(|old| old.name == entry.name)
        {
            *existing = entry;
        } else {
            registry
                .unpack
                .try_reserve(1)
                .map_err(|_| Error::allocation())?;
            registry.unpack.push(entry);
        }
        Ok(())
    });
    finish(result, ());
}

/// # Safety
/// The compiler supplies a static layout for (str, str) or (str, list[str], str).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_shutil_archive_formats(
    unpack: u32,
    layout: *const RecordDescriptor,
) -> *const Object {
    let result = REGISTRY.with_borrow(|registry| {
        let table = if unpack != 0 {
            &registry.unpack
        } else {
            &registry.pack
        };
        let mut ordered: Vec<&Entry> = Vec::new();
        ordered
            .try_reserve_exact(table.len())
            .map_err(|_| Error::allocation())?;
        ordered.extend(table);
        ordered.sort_unstable_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
        let mut result = Words::new(6);
        result
            .values
            .try_reserve(ordered.len())
            .map_err(|_| Error::allocation())?;
        for entry in ordered {
            // SAFETY: pointers written to the fresh tuple transfer one owner per field.
            unsafe {
                let row = allocate_record(layout);
                if row.is_null() {
                    return Err(Error::allocation());
                }
                row.write(Rc::into_raw(entry.name.clone()) as usize);
                if unpack != 0 {
                    row.add(1)
                        .write(Rc::into_raw(entry.extensions.clone()) as usize);
                }
                row.add(if unpack != 0 { 2 } else { 1 })
                    .write(Rc::into_raw(entry.description.clone()) as usize);
                result.values.push(row as usize);
            }
        }
        Ok(Rc::into_raw(result.into_object()))
    });
    finish(result, std::ptr::null())
}

/// # Safety
/// The caller roots the exact string name, including while a missing-key exception acquires it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_shutil_unregister_archive(name: *const Str, unpack: u32) {
    let result = REGISTRY.with_borrow_mut(|registry| {
        let table = if unpack != 0 {
            &mut registry.unpack
        } else {
            &mut registry.pack
        };
        // SAFETY: name is the caller's live string.
        let spelling = unsafe { &*name }.as_str();
        let position = table
            .iter()
            .position(|e| e.name.as_str() == spelling)
            .ok_or_else(|| {
                // SAFETY: one independent owner keeps the original key in KeyError.args.
                let key: OwnedWord = unsafe { WordKind::new(3).acquire(name as usize) };
                Error::key(key)
            })?;
        table.remove(position);
        Ok(())
    });
    finish(result, ())
}
