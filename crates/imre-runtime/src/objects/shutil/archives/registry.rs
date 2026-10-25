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
pub(super) enum Handler {
    Builtin(Format),
    Custom(Rc<Callback>),
}

#[derive(Clone)]
struct Entry {
    name: StrOwner,
    description: StrOwner,
    handler: Handler,
    suffix: &'static str,
    extensions: ObjectOwner,
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
                        .map(|&s| StrOwner::into_raw(StrOwner::new(Str::from(s))) as usize),
                );
                Entry {
                    name: StrOwner::new(Str::from(name)),
                    description: StrOwner::new(Str::from(description)),
                    handler: Handler::Builtin(format),
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

pub(super) fn archiver(name: &str) -> Option<(Handler, &'static str)> {
    REGISTRY.with_borrow(|registry| {
        registry
            .pack
            .iter()
            .find(|e| e.name.as_str() == name)
            .map(|e| (e.handler.clone(), e.suffix))
    })
}

pub(super) fn unpacker(name: Option<&str>, filename: &str) -> Option<Handler> {
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
            .map(|e| e.handler.clone())
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
    rest_kind: u32,
    positional_kind: u32,
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
                    .map(|(word, _)| StrOwner::acquire_word(word))
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
                name: StrOwner::acquire_word(name as usize),
                description: StrOwner::acquire_word(description as usize),
                extensions: ObjectOwner::acquire_word(extensions as usize),
                suffix: "",
                handler: Handler::Custom(Rc::new(Callback::new(
                    function,
                    adapter,
                    frame,
                    metadata,
                    extras,
                    extras_kind,
                    rest_kind,
                    positional_kind,
                ))),
            }
        };
        replace(&mut registry.unpack, entry)
    });
    finish(result, ());
}

fn replace(entries: &mut Vec<Entry>, entry: Entry) -> Result<()> {
    if let Some(existing) = entries.iter_mut().find(|old| old.name == entry.name) {
        *existing = entry;
    } else {
        entries.try_reserve(1).map_err(|_| Error::allocation())?;
        entries.push(entry);
    }
    Ok(())
}

/// # Safety
/// The compiler roots the arguments, and pairs each packing adapter with its callable,
/// frame layout and metadata. A null adapter denotes an evaluated non-callable.
/// A non-null root flag points to the function's static atomic byte.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_shutil_register_pack(
    name: *const Str,
    function: usize,
    function_kind: u32,
    extras: usize,
    extras_kind: u32,
    description: *const Str,
    adapter: *const (),
    frame: *const RecordDescriptor,
    metadata: *const usize,
    root_flag: *const std::sync::atomic::AtomicU8,
    rest_kind: u32,
    positional_kind: u32,
) {
    let result = REGISTRY.with_borrow_mut(|registry| -> Result<()> {
        if adapter.is_null() {
            let mut display = String::new();
            // SAFETY: both branches borrow the original operand using its checked representation.
            unsafe {
                if function_kind == 3 {
                    display.push_str(&*(function as *const Str));
                } else {
                    crate::objects::values::write_repr(
                        &mut display,
                        function,
                        WordKind::new(function_kind),
                    );
                }
            }
            return Err(Error::new(
                Kind::TypeError,
                format!("The {display} object is not callable"),
            ));
        }
        // SAFETY: the typed adapter and frame are static; the callback acquires all live records.
        let mut callback = unsafe {
            Callback::new(
                function as *const usize,
                adapter,
                frame,
                metadata,
                extras,
                extras_kind,
                rest_kind,
                positional_kind,
            )
        };
        // SAFETY: this flag, when present, lives for the entire native program.
        callback.root_flag = unsafe { root_flag.as_ref() };
        callback.check_pack_extras()?;
        let entry = Entry {
            // SAFETY: both strings are rooted by the caller.
            name: unsafe { StrOwner::acquire_word(name as usize) },
            description: unsafe { StrOwner::acquire_word(description as usize) },
            handler: Handler::Custom(Rc::new(callback)),
            suffix: "",
            extensions: Words::new(3).into_object(),
        };
        replace(&mut registry.pack, entry)
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
                row.write(StrOwner::into_raw(entry.name.clone()) as usize);
                if unpack != 0 {
                    row.add(1)
                        .write(ObjectOwner::into_raw(entry.extensions.clone()) as usize);
                }
                row.add(if unpack != 0 { 2 } else { 1 })
                    .write(StrOwner::into_raw(entry.description.clone()) as usize);
                result.values.push(row as usize);
            }
        }
        Ok(ObjectOwner::into_raw(result.into_object()))
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
