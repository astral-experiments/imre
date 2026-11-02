//! Linux child descriptor actions for posix_spawn and posix_spawnp.
use std::{ffi::CString, io};

use num_traits::ToPrimitive;

use super::super::{Context, Error, Id, NONE, Result, Value, builtins, os, os_error, posixpath};

const OPEN_ARITY: &str = "A open file_action tuple must have 5 elements";
const UNKNOWN: &str = "Unknown file_actions identifier";

pub(super) fn has_closefrom() -> bool {
    crate::objects::BoxedPopen::spawn_closes_fds()
}

fn check(code: libc::c_int) -> Result<()> {
    if code == 0 {
        Ok(())
    } else {
        Err(os_error(io::Error::from_raw_os_error(code)))
    }
}

// A conversion can remove the current action from its list or raise a Python-owned exception.
// Release borrowed operands before the next list lookup, preserving any exception through GC.
fn discard(ctx: &mut Context, mark: usize, error: Option<&Error>) {
    let collect = ctx.heap.discard_truth_operands(mark, ctx.modules);
    if let Some(error) = error {
        for root in error.roots() {
            ctx.heap.keep(root);
        }
    }
    if collect {
        ctx.collect_unreachable();
    }
}

/// Keep the initialized C object and the path strings it may borrow until the spawn finishes.
pub(super) struct FileActions {
    raw: Option<Box<libc::posix_spawn_file_actions_t>>,
    paths: Vec<CString>,
}

impl FileActions {
    pub(super) fn as_raw(&self) -> Option<&libc::posix_spawn_file_actions_t> {
        self.raw.as_deref()
    }

    pub(super) fn parse(ctx: &mut Context, source: Id) -> Result<Self> {
        let mark = ctx.heap.temporary_mark();
        let result = Self::from_source(ctx, source);
        discard(ctx, mark, result.as_ref().err());
        result
    }

    fn from_source(ctx: &mut Context, source: Id) -> Result<Self> {
        let mut actions = Self {
            raw: None,
            paths: Vec::new(),
        };
        if source == NONE {
            return Ok(actions);
        }

        // PySequence_Fast aliases exact lists/tuples; it consumes other iterables before parsing.
        let sequence = if matches!(ctx.heap.get(source), Value::List(_) | Value::Tuple(_)) {
            source
        } else {
            let iter = ctx.iterator(source).map_err(|err| {
                let ty = ctx.builtin("TypeError");
                if ctx.raised_error_matches(&err, ty) {
                    Error::new("TypeError", "file_actions must be a sequence or None")
                } else {
                    err
                }
            })?;
            let list = ctx.alloc(Value::List(Vec::new()));
            builtins::list_extend(ctx, list, iter)?;
            list
        };

        ctx.heap.reserve_temporaries(1)?;
        ctx.heap.keep(sequence);
        let mut raw = Box::<libc::posix_spawn_file_actions_t>::new_uninit();
        // SAFETY: the destination is writable, correctly aligned storage for the native object.
        check(unsafe { libc::posix_spawn_file_actions_init(raw.as_mut_ptr()) })?;
        // SAFETY: init succeeded and Drop will destroy the initialized object once.
        actions.raw = Some(unsafe { raw.assume_init() });

        let mut index = 0;
        while let Some((_, Some(item))) = ctx.sequence_at(sequence, index) {
            let mark = ctx.heap.temporary_mark();
            ctx.heap.reserve_temporaries(1)?;
            ctx.heap.keep(item);
            let appended = actions.append(ctx, item);
            discard(ctx, mark, appended.as_ref().err());
            appended?;
            index += 1;
        }
        Ok(actions)
    }

    fn append(&mut self, ctx: &mut Context, item: Id) -> Result<()> {
        let elements = ctx.tuple_items(item)?;
        let Some((&tag, fields)) = elements.as_deref().and_then(|tuple| tuple.split_first()) else {
            return Err(Error::new(
                "TypeError",
                "Each file_actions element must be a non-empty tuple",
            ));
        };
        let tag = ctx.index_c_long(tag)?;
        let raw = self.raw.as_deref_mut().expect("initialized file actions");
        let code = match (tag, fields) {
            (0, [fd, path, flags, mode]) => {
                let fd = ctx.index_c_int(*fd)?;
                let path = posixpath::fspath(ctx, *path)?;
                let path = os::c_path(ctx, path, "embedded null byte")?;
                let flags = ctx.index_c_int(*flags)?;
                let mode = ctx
                    .index_value(*mode)?
                    .ok_or_else(|| Error::new("TypeError", OPEN_ARITY))?;
                // getargs "k" masks to unsigned long; the kernel mode_t takes its low bits.
                let mode = (&*mode.big() & num_bigint::BigInt::from(libc::mode_t::MAX))
                    .to_u32()
                    .expect("masked Linux mode_t");
                self.paths
                    .try_reserve(1)
                    .map_err(|_| Error::new("MemoryError", ""))?;
                let filename = path.as_ptr();
                self.paths.push(path);
                // SAFETY: raw is initialized; path's NUL-terminated allocation survives the spawn.
                unsafe { libc::posix_spawn_file_actions_addopen(raw, fd, filename, flags, mode) }
            }
            (1, [fd]) => {
                let fd = ctx.index_c_int(*fd)?;
                // SAFETY: raw is initialized; libc validates the fd number and records the action.
                unsafe { libc::posix_spawn_file_actions_addclose(raw, fd) }
            }
            (2, [src, dst]) => {
                let src = ctx.index_c_int(*src)?;
                let dst = ctx.index_c_int(*dst)?;
                // SAFETY: both descriptor numbers are passed by value to the initialized action list.
                unsafe { libc::posix_spawn_file_actions_adddup2(raw, src, dst) }
            }
            (3, args) if has_closefrom() => {
                let [fd] = args else {
                    return Err(Error::new(
                        "TypeError",
                        "A closefrom file_action tuple must have 2 elements",
                    ));
                };
                let fd = ctx.index_c_int(*fd)?;
                // SAFETY: probe the optional GNU extension in the same process namespace as the capability test.
                let symbol = unsafe {
                    libc::dlsym(
                        libc::RTLD_DEFAULT,
                        c"posix_spawn_file_actions_addclosefrom_np".as_ptr(),
                    )
                };
                if symbol.is_null() {
                    return Err(Error::new("TypeError", UNKNOWN));
                }
                type AddCloseFrom = unsafe extern "C" fn(
                    *mut libc::posix_spawn_file_actions_t,
                    libc::c_int,
                ) -> libc::c_int;
                // SAFETY: this named libc function has the declared C ABI and signature.
                let add: AddCloseFrom = unsafe { std::mem::transmute(symbol) };
                // SAFETY: raw remains initialized and fd is passed by value.
                unsafe { add(raw, fd) }
            }
            (0, _) => return Err(Error::new("TypeError", OPEN_ARITY)),
            (1, _) => {
                return Err(Error::new(
                    "TypeError",
                    "A close file_action tuple must have 2 elements",
                ));
            }
            (2, _) => {
                return Err(Error::new(
                    "TypeError",
                    "A dup2 file_action tuple must have 3 elements",
                ));
            }
            _ => return Err(Error::new("TypeError", UNKNOWN)),
        };
        check(code)
    }
}

impl Drop for FileActions {
    fn drop(&mut self) {
        if let Some(raw) = self.raw.as_deref_mut() {
            // SAFETY: only successful initialization installs raw and its paths are still alive.
            unsafe { libc::posix_spawn_file_actions_destroy(raw) };
        }
    }
}
