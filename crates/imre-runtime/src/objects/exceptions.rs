//! Explicit language errors; no Rust unwind crosses the generated-code ABI.
//!
//! Pending and dynamically active errors own independent references. Their
//! Explicit causes form an immutable chain: the compiler attaches them only to
//! freshly constructed, unescaped exceptions. No traceback or mutable exception
//! attributes can introduce ownership cycles.

use std::cell::{Cell, RefCell};
use std::io::{self, Write};
use std::rc::Rc;

use num_traits::ToPrimitive;
use ruff_python_literal::escape::{Escape, UnicodeEscape};

use super::words::{OwnedWord, WordKind};
use super::{Object, Storage, StorageCell, Str, acquire_rc, fail, object};

#[repr(C)]
pub struct NamedDescriptor {
    name: *const u8,
    length: usize,
    kinds: *const u32,
    count: usize,
    base: u32,
}

/// Stable ABI identifiers for the supported builtin exception hierarchy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub(crate) enum Kind {
    BaseException = 0,
    Exception = 1,
    ArithmeticError = 2,
    ZeroDivisionError = 3,
    OverflowError = 4,
    LookupError = 5,
    IndexError = 6,
    KeyError = 7,
    ValueError = 8,
    AssertionError = 9,
    RuntimeError = 10,
    EOFError = 11,
    OSError = 12,
    TypeError = 13,
    AttributeError = 14,
    MemoryError = 15,
    SystemError = 16,
    UnicodeError = 17,
    UnicodeDecodeError = 18,
    UnsupportedOperation = 19,
    FileNotFoundError = 20,
    PermissionError = 21,
    IsADirectoryError = 22,
    NotADirectoryError = 23,
    FileExistsError = 24,
    SubprocessError = 25,
    CalledProcessError = 26,
    TimeoutExpired = 27,
    BadGzipFile = 28,
    ZlibError = 29,
    TomlDecodeError = 30,
    JsonDecodeError = 31,
    SystemExit = 32,
    CancelledError = 33,
    ExceptionGroup = 34,
    StopIteration = 35,
    BadZipFile = 36,
    NotImplementedError = 37,
    NameError = 39,
    UnboundLocalError = 40,
}

impl Kind {
    fn from_raw(kind: u32) -> Self {
        match kind {
            0 => Self::BaseException,
            1 => Self::Exception,
            2 => Self::ArithmeticError,
            3 => Self::ZeroDivisionError,
            4 => Self::OverflowError,
            5 => Self::LookupError,
            6 => Self::IndexError,
            7 => Self::KeyError,
            8 => Self::ValueError,
            9 => Self::AssertionError,
            10 => Self::RuntimeError,
            11 => Self::EOFError,
            12 => Self::OSError,
            13 => Self::TypeError,
            14 => Self::AttributeError,
            15 => Self::MemoryError,
            16 => Self::SystemError,
            17 => Self::UnicodeError,
            18 => Self::UnicodeDecodeError,
            19 => Self::UnsupportedOperation,
            20 => Self::FileNotFoundError,
            21 => Self::PermissionError,
            22 => Self::IsADirectoryError,
            23 => Self::NotADirectoryError,
            24 => Self::FileExistsError,
            25 => Self::SubprocessError,
            26 => Self::CalledProcessError,
            27 => Self::TimeoutExpired,
            28 => Self::BadGzipFile,
            29 => Self::ZlibError,
            30 => Self::TomlDecodeError,
            31 => Self::JsonDecodeError,
            32 => Self::SystemExit,
            33 => Self::CancelledError,
            34 => Self::ExceptionGroup,
            35 => Self::StopIteration,
            36 => Self::BadZipFile,
            37 => Self::NotImplementedError,
            39 => Self::NameError,
            40 => Self::UnboundLocalError,
            _ => fail("SystemError", "invalid exception kind"),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::BaseException => "BaseException",
            Self::Exception => "Exception",
            Self::ArithmeticError => "ArithmeticError",
            Self::ZeroDivisionError => "ZeroDivisionError",
            Self::OverflowError => "OverflowError",
            Self::LookupError => "LookupError",
            Self::IndexError => "IndexError",
            Self::KeyError => "KeyError",
            Self::ValueError => "ValueError",
            Self::AssertionError => "AssertionError",
            Self::RuntimeError => "RuntimeError",
            Self::EOFError => "EOFError",
            Self::OSError => "OSError",
            Self::TypeError => "TypeError",
            Self::AttributeError => "AttributeError",
            Self::MemoryError => "MemoryError",
            Self::SystemError => "SystemError",
            Self::UnicodeError => "UnicodeError",
            Self::UnicodeDecodeError => "UnicodeDecodeError",
            Self::UnsupportedOperation => "UnsupportedOperation",
            Self::FileNotFoundError => "FileNotFoundError",
            Self::PermissionError => "PermissionError",
            Self::IsADirectoryError => "IsADirectoryError",
            Self::NotADirectoryError => "NotADirectoryError",
            Self::FileExistsError => "FileExistsError",
            Self::SubprocessError => "SubprocessError",
            Self::CalledProcessError => "CalledProcessError",
            Self::TimeoutExpired => "TimeoutExpired",
            Self::BadGzipFile => "BadGzipFile",
            Self::ZlibError => "error",
            Self::TomlDecodeError => "TOMLDecodeError",
            Self::JsonDecodeError => "JSONDecodeError",
            Self::SystemExit => "SystemExit",
            Self::CancelledError => "CancelledError",
            Self::ExceptionGroup => "ExceptionGroup",
            Self::StopIteration => "StopIteration",
            Self::BadZipFile => "BadZipFile",
            Self::NotImplementedError => "NotImplementedError",
            Self::NameError => "NameError",
            Self::UnboundLocalError => "UnboundLocalError",
        }
    }

    fn matches(self, expected: Self) -> bool {
        match expected {
            Self::BaseException => true,
            Self::NameError => matches!(self, Self::NameError | Self::UnboundLocalError),
            Self::Exception => !matches!(
                self,
                Self::BaseException | Self::SystemExit | Self::CancelledError
            ),
            Self::ArithmeticError => matches!(
                self,
                Self::ArithmeticError | Self::ZeroDivisionError | Self::OverflowError
            ),
            Self::LookupError => {
                matches!(self, Self::LookupError | Self::IndexError | Self::KeyError)
            }
            Self::ValueError => matches!(
                self,
                Self::ValueError
                    | Self::JsonDecodeError
                    | Self::TomlDecodeError
                    | Self::UnicodeError
                    | Self::UnicodeDecodeError
                    | Self::UnsupportedOperation
            ),
            Self::UnicodeError => matches!(self, Self::UnicodeError | Self::UnicodeDecodeError),
            Self::SubprocessError => {
                matches!(
                    self,
                    Self::SubprocessError | Self::CalledProcessError | Self::TimeoutExpired
                )
            }
            Self::RuntimeError => matches!(self, Self::RuntimeError | Self::NotImplementedError),
            Self::OSError => matches!(
                self,
                Self::OSError
                    | Self::UnsupportedOperation
                    | Self::FileNotFoundError
                    | Self::PermissionError
                    | Self::IsADirectoryError
                    | Self::NotADirectoryError
                    | Self::FileExistsError
                    | Self::BadGzipFile
            ),
            _ => self == expected,
        }
    }
}

#[derive(Clone)]
pub(super) struct Exception {
    kind: Kind,
    payload: Payload,
    suppress_context: bool,
    details: Option<Rc<ExceptionDetails>>,
}

#[derive(Clone, Default)]
struct ExceptionDetails {
    cause: Option<Rc<Object>>,
    named: Option<&'static NamedDescriptor>,
    layout: Option<Rc<NamedPayload>>,
    directories: Option<Rc<DirectoryFrame>>,
}

type Formatter = unsafe extern "C" fn(*const usize) -> *const Str;
struct NamedPayload {
    record: OwnedWord,
    text: Option<Formatter>,
    repr: Option<Formatter>,
    dataclass_repr: bool,
    takes_record_owner: bool,
}
struct DirectoryFrame {
    managers: Vec<Rc<Object>>,
    outer: Option<Rc<DirectoryFrame>>,
}

impl Drop for DirectoryFrame {
    fn drop(&mut self) {
        let mut outer = self.outer.take();
        while let Some(mut frame) = outer.and_then(Rc::into_inner) {
            outer = frame.outer.take();
        }
    }
}

#[derive(Clone)]
enum Payload {
    Empty,
    Message(Rc<Str>),
    /// Some builtins render a human message that differs from their original arguments.
    Arguments(Box<BuiltinArguments>),
    Json(Rc<JsonError>),
    Key(OwnedWord),
    Exit(Option<OwnedWord>),
    Process(Box<super::processes::ProcessError>),
    Timeout(Box<super::processes::TimeoutError>),
    Group(Box<[GroupMember]>),
}

/// Indirect so retaining several arguments does not grow every Object allocation.
#[derive(Clone)]
struct BuiltinArguments {
    message: Rc<Str>,
    args: smallvec::SmallVec<[OwnedWord; 5]>,
}

struct JsonError {
    msg: Rc<Str>,
    doc: Rc<Str>,
    pos: usize,
    lineno: usize,
    colno: usize,
}

impl From<Rc<Str>> for Payload {
    fn from(message: Rc<Str>) -> Self {
        Self::Message(message)
    }
}

/// A member raised explicitly from its owning group needs both observable
/// directions without an Rc cycle. A weak identity is enough while anyone
/// holds the child. If only the group survives, its immutable value can be
/// materialized again; simultaneous projections preserve object identity.
#[derive(Clone)]
enum GroupMember {
    Strong(Rc<Object>),
    Caused(Rc<CausedMember>),
}

struct CausedMember {
    live: RefCell<std::rc::Weak<Object>>,
    value: Exception,
}

impl GroupMember {
    fn get(&self, owner: &Rc<Object>) -> Rc<Object> {
        match self {
            Self::Strong(child) => child.clone(),
            Self::Caused(entry) => {
                if let Some(child) = entry.live.borrow().upgrade() {
                    return child;
                }
                let mut value = entry.value.clone();
                value.details_mut().cause = Some(owner.clone());
                let child = Rc::new(Object(StorageCell::new(Storage::Exception(value))));
                *entry.live.borrow_mut() = Rc::downgrade(&child);
                child
            }
        }
    }
}

impl Exception {
    fn attach_directory_frame(&mut self, mut details: Rc<ExceptionDetails>) {
        if let Some(previous) = self.details.as_deref() {
            let fresh = Rc::get_mut(&mut details).unwrap();
            fresh.cause.clone_from(&previous.cause);
            fresh.named = previous.named;
            fresh.layout.clone_from(&previous.layout);
            Rc::get_mut(fresh.directories.as_mut().unwrap())
                .unwrap()
                .outer = previous.directories.clone();
        }
        self.details = Some(details);
    }

    fn details_mut(&mut self) -> &mut ExceptionDetails {
        Rc::make_mut(self.details.get_or_insert_with(Default::default))
    }

    fn cause(&self) -> Option<&Rc<Object>> {
        self.details.as_deref()?.cause.as_ref()
    }

    fn take_cause(&mut self) -> Option<Rc<Object>> {
        // A shared snapshot retains its own metadata and keeps the cause rooted.
        Rc::into_inner(self.details.take()?)?.cause
    }

    fn named(&self) -> Option<&'static NamedDescriptor> {
        self.details.as_deref()?.named
    }

    fn name(&self) -> &str {
        self.named().map_or_else(
            || self.kind.name(),
            |descriptor| {
                // SAFETY: the compiler emits a static UTF-8 identifier with this length.
                unsafe {
                    std::str::from_utf8_unchecked(std::slice::from_raw_parts(
                        descriptor.name,
                        descriptor.length,
                    ))
                }
            },
        )
    }
}

impl Drop for Exception {
    /// Release uniquely owned cause tails iteratively, including very long chains.
    fn drop(&mut self) {
        let mut next = self.take_cause();
        while let Some(cause) = next {
            let Ok(object) = Rc::try_unwrap(cause) else {
                break;
            };
            let Storage::Exception(mut exception) = object.0.into_inner() else {
                unreachable!("exception causes have exact exception storage");
            };
            next = exception.take_cause();
        }
    }
}

/// An owned error returned from a storage access before pending state is changed.
pub(crate) struct Error {
    kind: Kind,
    // A missing payload uses the preallocated MemoryError when raised.
    payload: Option<Payload>,
}

impl Error {
    pub(super) fn exit_status(status: i32) -> Self {
        let word = crate::Int::small(i64::from(status)).unwrap().into_raw();
        // SAFETY: the tagged integer transfers its owned word to the exception.
        let code = unsafe { OwnedWord::from_owned(word, WordKind::new(2)) };
        Self {
            kind: Kind::SystemExit,
            payload: Some(Payload::Exit(Some(code))),
        }
    }

    pub(crate) fn new(kind: Kind, message: impl Into<Str>) -> Self {
        Self {
            kind,
            payload: Some(Payload::Message(Rc::new(message.into()))),
        }
    }

    fn string_arg(text: impl Into<Str>) -> OwnedWord {
        let word = Rc::into_raw(Rc::new(text.into())) as usize;
        // SAFETY: kind 3 owns the transferred string allocation.
        unsafe { OwnedWord::from_owned(word, WordKind::new(3)) }
    }

    fn int_arg(value: crate::Int) -> OwnedWord {
        // SAFETY: kind 2 owns an integer word, whether tagged or allocated.
        unsafe { OwnedWord::from_owned(value.into_raw(), WordKind::new(2)) }
    }

    /// OSError retains its errno and strerror as args; a filename appears only in its message.
    pub(super) fn from_errno(kind: Kind, errno: i32, description: &str, message: String) -> Self {
        let args = [
            Self::int_arg(crate::Int::small(i64::from(errno)).unwrap()),
            Self::string_arg(description),
        ];
        Self {
            kind,
            payload: Some(Payload::Arguments(Box::new(BuiltinArguments {
                message: Rc::new(message.into()),
                args: args.into_iter().collect(),
            }))),
        }
    }

    /// UnicodeDecodeError preserves the undecoded bytes and the half-open error bounds.
    pub(super) fn unicode_decode(
        bytes: Rc<super::Bytes>,
        start: usize,
        end: usize,
        reason: &'static str,
        message: String,
    ) -> Self {
        // SAFETY: kind 8 owns the original immutable bytes, retained by the failing decoder.
        let object =
            unsafe { OwnedWord::from_owned(Rc::into_raw(bytes) as usize, WordKind::new(8)) };
        let args = [
            Self::string_arg("utf-8"),
            object,
            Self::int_arg(crate::Int::from_usize(start)),
            Self::int_arg(crate::Int::from_usize(end)),
            Self::string_arg(reason),
        ];
        Self {
            kind: Kind::UnicodeDecodeError,
            payload: Some(Payload::Arguments(Box::new(BuiltinArguments {
                message: Rc::new(message.into()),
                args: args.into_iter().collect(),
            }))),
        }
    }

    pub(super) fn is_kind(&self, kind: Kind) -> bool {
        self.kind == kind
    }

    pub(super) fn json_decode(
        msg: &str,
        doc: Rc<Str>,
        pos: usize,
        lineno: usize,
        colno: usize,
    ) -> Self {
        Self {
            kind: Kind::JsonDecodeError,
            payload: Some(Payload::Json(Rc::new(JsonError {
                msg: Rc::new(Str::from(msg)),
                doc,
                pos,
                lineno,
                colno,
            }))),
        }
    }

    pub(crate) fn allocation() -> Self {
        Self {
            kind: Kind::MemoryError,
            payload: None,
        }
    }

    pub(super) fn key(key: OwnedWord) -> Self {
        Self {
            kind: Kind::KeyError,
            payload: Some(Payload::Key(key)),
        }
    }

    pub(super) fn string_key(key: Str) -> Self {
        let word = Rc::into_raw(Rc::new(key)) as usize;
        // SAFETY: the new string contributes one owned reference of exact kind 3.
        Self::key(unsafe { OwnedWord::from_owned(word, WordKind::new(3)) })
    }

    pub(crate) fn index() -> Self {
        Self::new(Kind::IndexError, "list index out of range")
    }

    /// Installs this error only after any runtime storage access has ended.
    /// Keep exception allocation and cleanup out of successful access paths.
    #[cold]
    #[inline(never)]
    pub(crate) fn raise(self) {
        if let Some(payload) = self.payload {
            replace_pending(Some(new_exception(self.kind, payload)));
        } else {
            sequence_allocation_failed();
        }
    }

    /// Returns a failure sentinel after installing the owned exception.
    #[cold]
    #[inline(never)]
    pub(crate) fn returning<T>(self, sentinel: T) -> T {
        self.raise();
        sentinel
    }
}

pub(crate) type Result<T> = std::result::Result<T, Error>;

/// Converts an internal Result at the ABI boundary, after storage borrows end.
pub(crate) fn finish<T>(result: Result<T>, sentinel: T) -> T {
    result.unwrap_or_else(|error| error.returning(sentinel))
}

thread_local! {
    static RECORD_ALLOCATION_ERROR: RefCell<Rc<Object>> = RefCell::new(new_exception(
        Kind::MemoryError, Rc::new(Str::new("record allocation failed")),
    ));
    static SEQUENCE_ALLOCATION_ERROR: RefCell<Rc<Object>> = RefCell::new(new_exception(
        Kind::MemoryError, Rc::new(Str::new("sequence allocation failed")),
    ));
    static PENDING: RefCell<Option<Rc<Object>>> = const { RefCell::new(None) };
    static ACTIVE: RefCell<Option<Rc<Object>>> = const { RefCell::new(None) };
    // The non-Drop mirror lets successful calls query one TLS byte without
    // initializing or checking the owning cell's destructor/borrow state.
    // Only replace_pending/take_pending mutate the owner; both synchronize this
    // flag before dropping displaced owners. The single-thread, no-callback
    // contract makes the brief interval between owner and flag writes unobservable.
    static PENDING_PRESENT: Cell<bool> = const { Cell::new(false) };
}

/// Prepare allocation errors and exception state before generated code starts.
#[unsafe(no_mangle)]
pub extern "C" fn imre_record_error_init() {
    RECORD_ALLOCATION_ERROR.with(|_| ());
    SEQUENCE_ALLOCATION_ERROR.with(|_| ());
    PENDING.with(|_| ());
    ACTIVE.with(|_| ());
}

#[cold]
pub(super) fn record_allocation_failed() {
    RECORD_ALLOCATION_ERROR.with_borrow(|error| replace_pending(Some(Rc::clone(error))));
}

#[cold]
pub(crate) fn sequence_allocation_failed() {
    SEQUENCE_ALLOCATION_ERROR.with_borrow(|error| replace_pending(Some(Rc::clone(error))));
}

/// # Safety
/// The compiler supplies its current optional frame owner and a rooted nullable manager.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_prepare_directory_frame(
    frame: *const Object,
    manager: *const Object,
    count: usize,
) -> *const Object {
    // SAFETY: the caller roots the nullable manager while checking its cleanup state.
    if !frame.is_null() || !unsafe { armed_directory(manager) } {
        return frame;
    }
    let mut error = SEQUENCE_ALLOCATION_ERROR.with_borrow(|template| {
        // SAFETY: the template is a live immutable exception.
        unsafe { template.0.with(|storage| storage.exception().clone()) }
    });
    error.details = Some(Rc::new(ExceptionDetails {
        directories: Some(Rc::new(DirectoryFrame {
            managers: Vec::with_capacity(count),
            outer: None,
        })),
        ..ExceptionDetails::default()
    }));
    Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::Exception(error)))))
}

unsafe fn armed_directory(manager: *const Object) -> bool {
    if manager.is_null() {
        return false;
    }
    // SAFETY: the compiled local roots the present manager.
    unsafe {
        object(manager).0.with(|storage|
            matches!(storage, Storage::DirectoryContext(context) if context.needs_cleanup()))
    }
}

/// # Safety
/// The compiler owns the unpublished frame and visits at most its reserved number
/// of nullable directory-manager slots before transferring the frame.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_error_frame_keep_directory(
    frame: *const Object,
    manager: *const Object,
) {
    // SAFETY: the frame still owns each generated local it visits.
    if unsafe { armed_directory(manager) } {
        // SAFETY: frame is an exclusive, unpublished traceback shell.
        unsafe {
            object(frame).0.with_mut(|storage| {
                let Storage::Exception(error) = storage else {
                    unreachable!()
                };
                let roots = &mut Rc::get_mut(error.details_mut().directories.as_mut().unwrap())
                    .unwrap()
                    .managers;
                if !roots.iter().any(|root| Rc::as_ptr(root) == manager) {
                    roots.push(acquire_rc::<Object>(manager as usize));
                }
            })
        }
    }
}

/// # Safety
/// The caller transfers its single prepared frame reference after recording its managers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_error_attach_directory_frame(frame: *const Object) {
    // SAFETY: this consumes the compiler's one reserved frame owner.
    let frame = unsafe { Rc::from_raw(frame) };
    // SAFETY: the frame is still private to this unwind.
    let details = unsafe {
        frame.0.with_mut(|storage| {
            let Storage::Exception(error) = storage else {
                unreachable!()
            };
            error.details.take().unwrap()
        })
    };
    if details.directories.as_ref().unwrap().managers.is_empty() {
        return;
    }
    PENDING.with_borrow(|slot| {
        let Some(pending) = slot.as_ref() else { return };
        if let Some(template) = [&RECORD_ALLOCATION_ERROR, &SEQUENCE_ALLOCATION_ERROR]
            .into_iter()
            .find(|template| template.with_borrow(|cached| Rc::ptr_eq(cached, pending)))
        {
            // Move the spare into the cache. Existing aliases keep the exact pending
            // exception and its traceback, including an alias saved before a re-raise.
            // SAFETY: the cached allocation error has an immutable message payload.
            let mut pristine = unsafe { pending.0.with(|storage| storage.exception().clone()) };
            pristine.details = None;
            pristine.suppress_context = false;
            // SAFETY: the consumed frame shell is still unpublished.
            unsafe {
                frame
                    .0
                    .with_mut(|storage| *storage = Storage::Exception(pristine))
            };
            template.set(frame);
        }
        // SAFETY: attaching preallocated roots preserves the live exception identity.
        unsafe {
            pending.0.with_mut(|storage| {
                let Storage::Exception(error) = storage else {
                    unreachable!()
                };
                error.attach_directory_frame(details);
            })
        };
    });
}

fn new_exception(kind: Kind, payload: impl Into<Payload>) -> Rc<Object> {
    Rc::new(Object(StorageCell::new(Storage::Exception(Exception {
        kind,
        payload: payload.into(),
        suppress_context: false,
        details: None,
    }))))
}

pub(super) fn kind_of(error: &Object) -> Kind {
    // SAFETY: the task holds an immutable exact exception throughout the read.
    unsafe { error.0.with(Storage::exception_kind) }
}

pub(super) fn task_group_errors(errors: Vec<Rc<Object>>) -> Rc<Object> {
    new_exception(
        Kind::ExceptionGroup,
        Payload::Group(errors.into_iter().map(GroupMember::Strong).collect()),
    )
}

/// # Safety
/// The exact exception and canonical index stay rooted for the projection.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_async_exception_at(
    error: *const Object,
    index: usize,
) -> *const Object {
    // SAFETY: only an immutable child owner leaves the group's storage.
    let owner: Rc<Object> = unsafe { acquire_rc(error as usize) };
    // SAFETY: the owner roots the exact group while its child is acquired.
    let result = unsafe {
        owner.0.with(|storage| {
            let exception = storage.exception();
            let Payload::Group(group) = &exception.payload else {
                return Err(Error::new(
                    Kind::AttributeError,
                    format!(
                        "'{}' object has no attribute 'exceptions'",
                        exception.name()
                    ),
                ));
            };
            let index = super::decode_index(&crate::Int::borrow(index))
                .and_then(|index| super::index(index, group.len()))
                .map_err(|_| Error::new(Kind::IndexError, "tuple index out of range"))?;
            Ok(Rc::into_raw(group[index].get(&owner)))
        })
    };
    finish(result, std::ptr::null())
}

/// # Safety
/// The compiler supplies a child projected from this exact group; both stay
/// rooted during this transfer of relationship ownership.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_async_member_cause(
    child: *const Object,
    group: *const Object,
) -> *const Object {
    // SAFETY: two ordinary, rooted exception objects are accessed in separate
    // scopes. No user code can reenter their storage during ownership transfer.
    unsafe {
        let child: Rc<Object> = acquire_rc(child as usize);
        let group: Rc<Object> = acquire_rc(group as usize);
        let mut snapshot = child.0.with(|storage| storage.exception().clone());
        if snapshot.cause().is_some() {
            snapshot.details_mut().cause = None;
        }
        snapshot.suppress_context = true;
        let entry = Rc::new(CausedMember {
            live: RefCell::new(Rc::downgrade(&child)),
            value: snapshot,
        });
        group.0.with_mut(|storage| {
            let Storage::Exception(parent) = storage else {
                unreachable!()
            };
            let Payload::Group(members) = &mut parent.payload else {
                unreachable!("child belongs to group");
            };
            for member in members.iter_mut() {
                if matches!(member, GroupMember::Strong(target) if Rc::ptr_eq(target, &child)) {
                    *member = GroupMember::Caused(entry.clone());
                }
            }
        });
        child.0.with_mut(|storage| {
            let Storage::Exception(value) = storage else {
                unreachable!()
            };
            value.details_mut().cause = Some(group);
            value.suppress_context = true;
        });
        Rc::into_raw(child)
    }
}

/// Publishes a completed child's rich failure after all input storage borrows end.
#[cold]
pub(super) fn raise_process(error: super::processes::ProcessError) {
    replace_pending(Some(new_exception(
        Kind::CalledProcessError,
        Payload::Process(Box::new(error)),
    )));
}

/// Publishes the immutable deadline snapshot after the child has been reaped.
#[cold]
pub(super) fn raise_timeout(error: super::processes::TimeoutError) {
    replace_pending(Some(new_exception(
        Kind::TimeoutExpired,
        Payload::Timeout(Box::new(error)),
    )));
}

/// Replaces the pending owner without dropping it during a TLS mutable borrow.
pub(super) fn replace_pending(value: Option<Rc<Object>>) {
    let present = value.is_some();
    let previous = PENDING.with(|pending| pending.replace(value));
    PENDING_PRESENT.set(present);
    drop(previous);
}

/// Clears presence before transferring the owner, without dropping under a TLS borrow.
pub(super) fn take_pending() -> Option<Rc<Object>> {
    let pending = PENDING.with(RefCell::take);
    PENDING_PRESENT.set(false);
    pending
}

/// A suspended task retains the exception dynamically active in its own handlers.
pub(super) fn swap_active(saved: &mut Option<Rc<Object>>) {
    ACTIVE.with(|active| std::mem::swap(&mut *active.borrow_mut(), saved));
}

pub(super) fn active_exception() -> Option<Rc<Object>> {
    ACTIVE.with(|active| active.borrow().clone())
}

/// Uses Ruff's Python repr rules and its pinned Unicode printable categories.
pub(crate) fn string_repr(value: &str) -> Str {
    let mut result = String::new();
    push_string_repr(value, &mut result);
    Str::from(result)
}

pub(super) fn push_string_repr(value: &str, out: &mut String) {
    let escaped = UnicodeEscape::new_repr(value);
    if out.is_empty()
        && let Some(size) = escaped.layout().len.and_then(|body| body.checked_add(2))
    {
        out.reserve_exact(size);
    }
    write_escaped_string(&escaped, out).expect("writing to a String cannot fail");
}

pub(crate) fn write_string_repr(value: &str, out: &mut impl std::fmt::Write) -> std::fmt::Result {
    write_escaped_string(&UnicodeEscape::new_repr(value), out)
}

fn write_escaped_string(
    escaped: &UnicodeEscape<'_>,
    out: &mut impl std::fmt::Write,
) -> std::fmt::Result {
    let quote = escaped.layout().quote.as_char();
    out.write_char(quote)?;
    escaped.write_body(out)?;
    out.write_char(quote)
}

/// Acquires nullable exception input without retaining a storage reference.
unsafe fn acquire_exception(value: *const Object) -> Option<Rc<Object>> {
    if value.is_null() {
        return None;
    }
    // SAFETY: the caller roots the object; only immutable kind metadata escapes.
    unsafe { object(value).0.with(Storage::exception_kind) };
    // SAFETY: the shape-checked handle remains rooted throughout acquisition.
    Some(unsafe { acquire_rc(value as usize) })
}

impl Storage {
    fn exception(&self) -> &Exception {
        match self {
            Self::Exception(exception) => exception,
            _ => fail("SystemError", "expected an exception object"),
        }
    }

    fn exception_kind(&self) -> Kind {
        self.exception().kind
    }

    fn exception_message(&self) -> Rc<Str> {
        match &self.exception().payload {
            Payload::Exit(code) => exit_text(code.as_ref(), ""),
            Payload::Empty => Rc::new(Str::new("")),
            Payload::Message(message) => Rc::clone(message),
            Payload::Arguments(arguments) => Rc::clone(&arguments.message),
            Payload::Json(e) => Rc::new(Str::from(format!(
                "{}: line {} column {} (char {})",
                e.msg, e.lineno, e.colno, e.pos
            ))),
            Payload::Process(process) => Rc::new(process.message()),
            Payload::Timeout(timeout) => Rc::new(timeout.message()),
            Payload::Key(key) if key.kind == WordKind::new(3) => {
                // SAFETY: this owned kind-3 word is a live string.
                Rc::new(string_repr(unsafe { &*(key.word as *const Str) }))
            }
            Payload::Key(key) => {
                let mut message = String::new();
                // SAFETY: the exception owns the immutable key and its exact kind.
                unsafe { super::values::write_repr(&mut message, key.word, key.kind) };
                Rc::new(Str::from(message))
            }
            Payload::Group(members) => {
                let count = members.len();
                let suffix = if count == 1 { "" } else { "s" };
                Rc::new(Str::from(format!(
                    "unhandled errors in a TaskGroup ({count} sub-exception{suffix})"
                )))
            }
        }
    }
}

/// # Safety
/// The caller roots an exception and supplies the field selector emitted by the
/// compiler. On success, string and integer fields transfer independent owners.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_json_error_field(value: *const Object, field: u32) -> usize {
    // SAFETY: the exception stays live until the field has been acquired.
    let result = unsafe {
        object(value).0.with(|storage| {
            let exception = storage.exception();
            if let Payload::Json(e) = &exception.payload {
                return Ok(match field {
                    0 => Rc::into_raw(e.msg.clone()) as usize,
                    1 => Rc::into_raw(e.doc.clone()) as usize,
                    2 => crate::Int::from_usize(e.pos).into_raw(),
                    3 => crate::Int::from_usize(e.lineno).into_raw(),
                    4 => crate::Int::from_usize(e.colno).into_raw(),
                    _ => unreachable!("invalid JSON exception field"),
                });
            }
            let name = ["msg", "doc", "pos", "lineno", "colno"][field as usize];
            Err(Error::new(
                Kind::AttributeError,
                format!("'{}' object has no attribute '{name}'", exception.name()),
            ))
        })
    };
    finish(result, 0)
}

fn exit_text(code: Option<&OwnedWord>, missing: &str) -> Rc<Str> {
    let Some(code) = code else {
        return Rc::new(Str::new(missing));
    };
    if code.kind == WordKind::new(3) {
        // SAFETY: the exit payload roots this string.
        return unsafe { acquire_rc(code.word) };
    }
    let mut text = String::new();
    // SAFETY: the payload owns a scalar with this exact word kind.
    unsafe { super::values::write_repr(&mut text, code.word, code.kind) };
    Rc::new(Str::from(text))
}

/// Construct SystemExit with its original code. Kind 4 denotes an omitted
/// argument; 0, 1, 2 and 3 represent None, bool, int and str respectively.
///
/// # Safety
/// The caller roots the scalar or immutable string described by `kind`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_system_exit_new(kind: u32, word: usize) -> *const Object {
    let code = (kind != 4).then(|| {
        // SAFETY: the caller roots the argument through this acquisition.
        unsafe { WordKind::new(kind).acquire(word) }
    });
    Rc::into_raw(new_exception(Kind::SystemExit, Payload::Exit(code)))
}

/// Acquire an immutable view of the heterogeneous `SystemExit.code` scalar.
///
/// # Safety
/// `value` is a rooted builtin exception.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_system_exit_code(value: *const Object) -> *const Object {
    // SAFETY: the caller roots the immutable exception.
    let kind = unsafe { (*value).0.with(|s| s.exception().kind) };
    if kind != Kind::SystemExit {
        return Error::new(
            Kind::AttributeError,
            format!("'{}' object has no attribute 'code'", kind.name()),
        )
        .returning(std::ptr::null());
    }
    // SAFETY: the returned owner keeps the original exception and code alive.
    Rc::into_raw(unsafe { acquire_rc(value as usize) })
}

unsafe fn exit_word(value: *const Object) -> (WordKind, usize) {
    // SAFETY: a code view keeps its immutable typed exit payload alive.
    unsafe {
        (*value).0.with(|s| match &s.exception().payload {
            Payload::Exit(Some(code)) => (code.kind, code.word),
            Payload::Exit(None) => (WordKind::new(0), 0),
            _ => unreachable!("checked SystemExit code view"),
        })
    }
}

/// Identify the scalar type for an isinstance check on the original code.
///
/// # Safety
/// The caller roots a code view from `imre_system_exit_code`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_system_exit_code_kind(value: *const Object) -> u32 {
    // SAFETY: the code view roots its typed payload.
    unsafe { exit_word(value).0.raw() }
}

/// Render an exit code as a Python value, including None for a missing argument.
///
/// # Safety
/// `value` is a rooted code view returned by `imre_system_exit_code`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_system_exit_code_text(value: *const Object) -> *const Str {
    // SAFETY: the caller roots the immutable exit payload.
    unsafe {
        (*value).0.with(|s| {
            let Payload::Exit(code) = &s.exception().payload else {
                unreachable!()
            };
            Rc::into_raw(exit_text(code.as_ref(), "None"))
        })
    }
}

/// Compare the original exit code against a scalar or a second code view.
///
/// # Safety
/// `value` is a rooted code view. `word` is rooted with kind 0 (None), 1
/// (bool), 2 (int), 3 (str), or 5 (another code view).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_system_exit_code_equal(
    value: *const Object,
    kind: u32,
    word: usize,
) -> u32 {
    fn numeric(kind: WordKind, word: usize) -> (WordKind, usize) {
        if kind == WordKind::new(1) {
            (WordKind::new(2), (word << 1) | 1)
        } else {
            (kind, word)
        }
    }
    // SAFETY: both code views and all inferred scalar arguments remain rooted.
    let ((left_kind, left), (right_kind, right)) = unsafe {
        let (k, w) = exit_word(value);
        let (j, v) = if kind == 5 {
            exit_word(word as *const Object)
        } else {
            (WordKind::new(kind), word)
        };
        (numeric(k, w), numeric(j, v))
    };
    // SAFETY: matching immutable scalar kinds describe both words.
    u32::from(left_kind == right_kind && unsafe { super::values::equal(left, right, left_kind) })
}

/// # Safety
/// `value` is a rooted immutable exit code.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_system_exit_code_truth(value: *const Object) -> u32 {
    // SAFETY: the code view roots the scalar described by the stored word kind.
    let (kind, word) = unsafe { exit_word(value) };
    u32::from(match kind.raw() {
        0 => false,
        1 => word != 0,
        2 => word != 1, // tagged zero
        // SAFETY: the immutable exit code owns this string.
        3 => !unsafe { &*(word as *const Str) }.is_empty(),
        _ => unreachable!(),
    })
}

/// Acquires one sealed process-error field: cmd, returncode, stdout, or stderr;
/// field 4 is the output alias of stdout and preserves its attribute error name.
/// An ordinary exception receiver raises AttributeError instead of exposing storage.
///
/// # Safety
/// `value` is a rooted exception, `field` is 0 through 4, and pending is clear.
/// Bit 3 selects a tagged str/bytes representation for heterogeneous capture sites.
/// Bit 4 allows timeout stream snapshots when the caller's field can represent them.
/// Success returns the acquired argv, integer, or correctly typed stream field. Check pending
/// before interpreting zero, then release the result using that exact kind.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_subprocess_error_field(value: *const Object, field: u32) -> usize {
    let tagged = field & 8 != 0;
    let timeouts = field & 16 != 0;
    let field = field & !(8 | 16);
    if field > 4 {
        fail("SystemError", "invalid process exception field");
    }
    // SAFETY: only an acquired word or owned error leaves this rooted storage read.
    let result = unsafe {
        object(value).0.with(|storage| {
            let exception = storage.exception();
            match &exception.payload {
                Payload::Process(process) => Ok(process.field(field, tagged)),
                Payload::Timeout(timeout) if field == 0 => Ok(timeout.field(0)),
                Payload::Timeout(timeout) if timeouts && (2..=4).contains(&field) => {
                    let word = timeout.field(field);
                    Ok(if word != 0 && tagged { word | 1 } else { word })
                }
                _ => {
                    let name = ["cmd", "returncode", "stdout", "stderr", "output"][field as usize];
                    Err(Error::new(
                        Kind::AttributeError,
                        format!("'{}' object has no attribute '{name}'", exception.name()),
                    ))
                }
            }
        })
    };
    finish(result, 0)
}

/// Acquires a field from a proved TimeoutExpired view without changing its type.
///
/// # Safety
/// `value` is a rooted TimeoutExpired and `field` is 0 through 4. The returned
/// owned word has kind 5, integer or float, 264, 264, or 264 respectively.
/// A seconds read requires a program-wide proof of its numeric kind.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_subprocess_timeout_field(value: *const Object, field: u32) -> usize {
    // SAFETY: the checker establishes an exact TimeoutExpired before projection.
    unsafe {
        object(value).0.with(|storage| {
            let Payload::Timeout(timeout) = &storage.exception().payload else {
                fail("SystemError", "expected TimeoutExpired storage");
            };
            timeout.field(field)
        })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_error_pending() -> u32 {
    u32::from(PENDING_PRESENT.get())
}

/// Transfers the pending reference to the caller and clears pending state.
#[unsafe(no_mangle)]
pub extern "C" fn imre_error_take() -> *const Object {
    take_pending().map_or(std::ptr::null(), Rc::into_raw)
}

/// # Safety
/// `value` is a rooted exception or null. This borrows/acquires it; null clears
/// pending state. Cleanup and error access remain valid while an error is pending.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_error_set(value: *const Object) {
    // SAFETY: the caller supplies a rooted exception or null.
    replace_pending(unsafe { acquire_exception(value) });
}

/// # Safety
/// The descriptor and its name/ancestor arrays live for the process lifetime.
/// The nullable message is rooted exactly as for a builtin exception.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_named_exception_new(
    descriptor: *const NamedDescriptor,
    message: *const Str,
) -> *const Object {
    // SAFETY: the fresh builtin has not escaped and the descriptor is static.
    unsafe {
        let descriptor = &*descriptor;
        let handle = imre_exception_new(descriptor.base, message);
        object(handle).0.with_mut(|storage| {
            let Storage::Exception(exception) = storage else {
                unreachable!()
            };
            exception.details_mut().named = Some(descriptor);
        });
        handle
    }
}

/// Construct an error with a single non-string argument. Formatting happens
/// when observed, so mutation of a retained container remains visible.
///
/// # Safety
/// The word and optional static subtype descriptor have their exact declared kinds.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_exception_argument(
    kind: u32,
    descriptor: *const NamedDescriptor,
    word: usize,
    representation: u32,
) -> *const Object {
    // SAFETY: the caller roots the argument until this error owns its reference.
    unsafe {
        let named = descriptor.as_ref();
        let kind = Kind::from_raw(named.map_or(kind, |d| d.base));
        let value = new_exception(
            kind,
            Payload::Key(WordKind::new(representation).acquire(word)),
        );
        if let Some(named) = named {
            value.0.with_mut(|s| {
                let Storage::Exception(e) = s else {
                    unreachable!()
                };
                e.details_mut().named = Some(named);
            });
        }
        Rc::into_raw(value)
    }
}

/// Attach the completed Python field layout to the canonical exception object.
///
/// # Safety
/// The descriptor and method addresses are static. The compiler supplies a rooted
/// complete record, its string argument (or null), and unary record-to-str methods.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_named_exception_record_new(
    descriptor: *const NamedDescriptor,
    record: *const usize,
    message: *const Str,
    text: Option<Formatter>,
    repr: Option<Formatter>,
    display_flags: u32,
) -> *const Object {
    // SAFETY: the fresh exception cannot yet alias user code and its payload
    // acquires the exact native record before the caller releases that owner.
    unsafe {
        let value = imre_named_exception_new(descriptor, message);
        let payload = NamedPayload {
            record: WordKind::new(6).acquire(record as usize),
            text,
            repr,
            dataclass_repr: display_flags & 1 != 0,
            takes_record_owner: display_flags & 2 != 0,
        };
        object(value).0.with_mut(|s| {
            let Storage::Exception(e) = s else {
                unreachable!()
            };
            e.details_mut().layout = Some(Rc::new(payload));
        });
        value
    }
}

/// Project the fixed record only after a static or dynamically matched nominal type.
///
/// # Safety
/// The compiler proves the rooted error owns the payload's exact fixed layout.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_named_exception_record(value: *const Object) -> *const usize {
    // SAFETY: retaining the record does not reenter Python or mutate the exception.
    unsafe {
        object(value).0.with(|s| {
            s.exception()
                .details
                .as_ref()
                .unwrap()
                .layout
                .as_ref()
                .unwrap()
                .record
                .clone()
                .into_word() as *const usize
        })
    }
}

/// Format after the storage borrow ends: a Python display method may read aliases,
/// mutate its fields, or raise. The payload stays live through the callback.
unsafe fn display(value: *const Object, repr: bool) -> *const Str {
    // SAFETY: acquire the payload owner before invoking native Python methods.
    let layout = unsafe {
        object(value).0.with(|s| {
            s.exception()
                .details
                .as_ref()
                .and_then(|d| d.layout.clone())
        })
    };
    if let Some(layout) = layout {
        if let Some(method) = if repr { layout.repr } else { layout.text } {
            // SAFETY: the compiler checked the descriptor's unary record-to-str ABI.
            if layout.takes_record_owner {
                // The generated function consumes resource-bearing arguments on
                // all exits; other record parameters borrow for the call.
                let _ = layout.record.clone().into_word();
            }
            // SAFETY: our layout or the transferred reference roots the record.
            return unsafe { method(layout.record.word as *const usize) };
        }
        if repr && layout.dataclass_repr {
            // SAFETY: the record is owned and its descriptor contains the dataclass repr schema.
            return unsafe { super::values::imre_value_repr(layout.record.word, 6) };
        }
    }
    // SAFETY: base message formatting uses only sealed immutable builtin payloads.
    unsafe {
        object(value).0.with(|s| {
            if !repr {
                return Rc::into_raw(s.exception_message());
            }
            let e = s.exception();
            let arg = match &e.payload {
                Payload::Empty => Str::new(""),
                Payload::Arguments(arguments) => {
                    let mut result = String::new();
                    for (index, arg) in arguments.args.iter().enumerate() {
                        if index != 0 {
                            result.push_str(", ");
                        }
                        super::values::write_repr(&mut result, arg.word, arg.kind);
                    }
                    result.into()
                }
                Payload::Key(k) => {
                    let mut s = String::new();
                    super::values::write_repr(&mut s, k.word, k.kind);
                    s.into()
                }
                Payload::Exit(None) => Str::new(""),
                Payload::Exit(Some(k)) => {
                    let mut s = String::new();
                    super::values::write_repr(&mut s, k.word, k.kind);
                    s.into()
                }
                _ => string_repr(&s.exception_message()),
            };
            Rc::into_raw(Rc::new(format!("{}({})", e.name(), arg).into()))
        })
    }
}

/// Return the current repr of a nominal exception's fields or builtin arguments.
///
/// # Safety
/// The caller roots an exception and checks pending state if its method raises.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_exception_repr(value: *const Object) -> *const Str {
    // SAFETY: the caller roots the exception until its method returns.
    unsafe { display(value, true) }
}

/// Constructs an owned builtin exception from zero or one string argument.
/// A null message means zero arguments; KeyError's one-string form uses repr.
///
/// # Safety
/// `kind` is a supported builtin kind. `message` is a rooted string or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_exception_new(kind: u32, message: *const Str) -> *const Object {
    let kind = Kind::from_raw(kind);
    let payload = if message.is_null() {
        Payload::Empty
    } else if kind == Kind::KeyError {
        // SAFETY: the caller roots the immutable string argument.
        Payload::Key(unsafe { WordKind::new(3).acquire(message as usize) })
    } else {
        // SAFETY: the non-null argument is a rooted Rc<Str>.
        Payload::Message(unsafe { acquire_rc(message as usize) })
    };
    Rc::into_raw(new_exception(kind, payload))
}

/// # Safety
/// `value` is a rooted exception or null; acquires one additional reference.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_exception_retain(value: *const Object) {
    // SAFETY: exceptions use the ordinary Object ownership representation.
    unsafe { super::imre_object_retain(value) };
}

/// # Safety
/// Consumes one owned exception reference, or accepts null during cleanup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_exception_release(value: *const Object) {
    // SAFETY: exceptions use the ordinary Object ownership representation.
    unsafe { super::imre_object_release(value) };
}

/// # Safety
/// `value` is a rooted exception and `expected_kind` is a supported builtin kind.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_exception_matches(value: *const Object, expected_kind: u32) -> u32 {
    // SAFETY: descriptors have static storage; the compiler validates their MRO.
    unsafe {
        object(value).0.with(|storage| {
            let e = storage.exception();
            if let Some(d) = e.named()
                && std::slice::from_raw_parts(d.kinds, d.count).contains(&expected_kind)
            {
                return 1;
            }
            u32::from(expected_kind < 256 && e.kind.matches(Kind::from_raw(expected_kind)))
        })
    }
}

/// Returns an owned string containing Python's str(exception) representation.
///
/// # Safety
/// `value` must be a rooted exception object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_exception_message(value: *const Object) -> *const Str {
    // SAFETY: the clone owns its immutable message independently of the exception.
    unsafe { display(value, false) }
}

/// Attaches an explicit cause before publishing a fresh exception and returns
/// an additional owned reference to that same exception. Null represents `from None`.
///
/// # Safety
/// `value` is a fresh, unescaped exception, and `cause` is a rooted exception or
/// null. The cause cannot contain `value`; neither input is consumed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_exception_with_cause(
    value: *const Object,
    cause: *const Object,
) -> *const Object {
    // SAFETY: the caller roots the nullable exception cause.
    let cause = unsafe { acquire_exception(cause) };
    // SAFETY: the fresh object cannot alias a cause, and no storage borrow escapes.
    unsafe {
        object(value).0.with_mut(|storage| {
            let Storage::Exception(exception) = storage else {
                fail("SystemError", "expected an exception object");
            };
            debug_assert!(exception.cause().is_none() && !exception.suppress_context);
            if let Some(cause) = cause {
                exception.details_mut().cause = Some(cause);
            }
            exception.suppress_context = true;
        });
        imre_exception_retain(value);
    }
    value
}

/// Returns an independently owned explicit cause, or null when absent.
///
/// # Safety
/// `value` is a rooted exception.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_exception_cause(value: *const Object) -> *const Object {
    // SAFETY: the returned clone owns the cause after storage access ends.
    unsafe {
        object(value)
            .0
            .with(|storage| storage.exception().cause().cloned())
            .map_or(std::ptr::null(), Rc::into_raw)
    }
}

/// # Safety
/// `value` is a rooted exception.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_exception_suppress_context(value: *const Object) -> u32 {
    // SAFETY: only immutable metadata escapes storage access.
    unsafe {
        object(value)
            .0
            .with(|storage| u32::from(storage.exception().suppress_context))
    }
}

/// Acquires the dynamically active exception, or returns null outside a handler.
#[unsafe(no_mangle)]
pub extern "C" fn imre_exception_current() -> *const Object {
    ACTIVE.with(|active| {
        active
            .borrow()
            .clone()
            .map_or(std::ptr::null(), Rc::into_raw)
    })
}

/// Borrows/acquires the incoming active exception and transfers the previous
/// owner to the caller. Nested handlers save and restore that owner explicitly.
///
/// # Safety
/// `value` is a rooted exception or null; the returned reference must be released.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_exception_swap_active(value: *const Object) -> *const Object {
    // SAFETY: the caller roots the nullable exception before TLS replacement.
    let new = unsafe { acquire_exception(value) };
    ACTIVE.with(|active| active.replace(new).map_or(std::ptr::null(), Rc::into_raw))
}

/// # Safety
/// `value` must be a rooted, non-null exception. Pending state acquires a reference.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_exception_raise(value: *const Object) {
    if value.is_null() {
        fail("SystemError", "cannot raise a null exception");
    }
    // SAFETY: the caller supplies a rooted exception.
    unsafe { imre_error_set(value) };
}

/// Bare raise observes the dynamically active handler, including across calls.
#[unsafe(no_mangle)]
pub extern "C" fn imre_exception_reraise() {
    let active = ACTIVE.with(|active| active.borrow().clone());
    if let Some(active) = active {
        replace_pending(Some(active));
    } else {
        Error::new(Kind::RuntimeError, "No active exception to reraise").raise();
    }
}

/// Reports and consumes the pending error after generated-code cleanup. Returning
/// the exit status lets the C entry point finish normally, including sanitizer
/// teardown. A failed best-effort stdout flush preserves the original error.
#[unsafe(no_mangle)]
pub extern "C" fn imre_error_unhandled() -> i32 {
    let pending = take_pending();
    let active = ACTIVE.with(RefCell::take);
    drop(active);
    let Some(pending) = pending else {
        return 0;
    };
    // SAFETY: pending roots its immutable exception payload through this visit.
    let exit = unsafe {
        pending.0.with(|storage| {
            let Payload::Exit(code) = &storage.exception().payload else {
                return None;
            };
            let (status, message) = match code {
                None => (0, None),
                Some(code) if code.kind == WordKind::new(0) => (0, None),
                Some(code) if code.kind == WordKind::new(1) => (code.word as i32, None),
                Some(code) if code.kind == WordKind::new(2) => {
                    // CPython converts through signed long long, using -1 on overflow.
                    let integer = crate::Int::borrow(code.word);
                    (integer.big().to_i64().unwrap_or(-1) as i32, None)
                }
                Some(code) => (1, Some(exit_text(Some(code), ""))),
            };
            Some((status, message))
        })
    };
    if let Some((status, message)) = exit {
        let _ = crate::flush_stdout();
        let _ = crate::sys::flush_stderr();
        if let Some(text) = message {
            let _ = writeln!(io::stderr().lock(), "{text}");
        }
        return status;
    }
    let mut chain = Vec::new();
    let mut current = Some(pending.clone());
    while let Some(exception) = current {
        // SAFETY: the current owner roots the exception; only owned data escapes.
        let (name, cause) = unsafe {
            exception.0.with(|storage| {
                let e = storage.exception();
                (e.name().to_owned(), e.cause().cloned())
            })
        };
        // SAFETY: the local owner outlives custom formatting and any resulting error.
        let message = unsafe { display(Rc::as_ptr(&exception), false) };
        let message = if message.is_null() {
            drop(take_pending());
            Rc::new(Str::new("<exception str() failed>"))
        } else {
            // SAFETY: display transfers one owned string.
            unsafe { Rc::from_raw(message) }
        };
        chain.push((name, message));
        current = cause;
    }
    drop(pending);
    let _ = crate::flush_stdout();
    let _ = crate::sys::flush_stderr();
    let mut stderr = io::stderr().lock();
    for (index, (name, message)) in chain.into_iter().rev().enumerate() {
        if index != 0 {
            let _ = writeln!(
                stderr,
                "\nThe above exception was the direct cause of the following exception:\n"
            );
        }
        if message.is_empty() {
            let _ = writeln!(stderr, "{name}");
        } else {
            let _ = writeln!(stderr, "{name}: {message}");
        }
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::mappings::{
        imre_dict_delete, imre_dict_get, imre_dict_iter, imre_dict_new, imre_dict_set,
        imre_mapping_iter_next,
    };
    use crate::objects::{
        imre_seq_append, imre_seq_get_index, imre_seq_new, imre_seq_pop, imre_seq_set_index,
    };
    use crate::{
        Int, imre_int_binary, imre_int_parse, imre_list_append, imre_list_copy_index,
        imre_list_get_index, imre_list_new, imre_list_pop, imre_list_repeat,
    };
    use num_bigint::BigInt;

    #[test]
    fn cached_memory_errors_keep_separate_directory_owners() {
        use crate::objects::tempdirs::{imre_directory_context_new, imre_tempdir_new};

        fn retained(raise: fn()) -> (Rc<Object>, Rc<Str>) {
            let prefix = Str::from("imre-error-");
            let suffix = Str::from("");
            // SAFETY: both names are rooted and a null directory selects the system default.
            let name = unsafe { imre_tempdir_new(std::ptr::null(), &prefix, &suffix) };
            assert!(!name.is_null());
            // SAFETY: creation transfers one string owner. The manager acquires its own reference.
            let (name, manager) = unsafe {
                let manager = imre_directory_context_new(name as usize, 1);
                (Rc::from_raw(name), Rc::from_raw(manager))
            };
            // SAFETY: the compiler roots the manager while preparing its one traceback slot.
            let frame =
                unsafe { imre_prepare_directory_frame(std::ptr::null(), Rc::as_ptr(&manager), 1) };
            raise();
            let alias = PENDING.with_borrow(|pending| pending.as_ref().unwrap().clone());
            // SAFETY: the compiler transfers one private frame after visiting its single local.
            unsafe {
                imre_error_frame_keep_directory(frame, Rc::as_ptr(&manager));
                imre_error_attach_directory_frame(frame);
            }
            let error = take_pending().unwrap();
            assert!(Rc::ptr_eq(&alias, &error));
            (error, name)
        }

        for raise in [record_allocation_failed as fn(), sequence_allocation_failed] {
            let (first, left) = retained(raise);
            let (second, right) = retained(raise);
            assert!(!Rc::ptr_eq(&first, &second));
            let left = std::path::Path::new(left.as_str());
            let right = std::path::Path::new(right.as_str());
            assert!(left.is_dir() && right.is_dir());
            drop(first);
            assert!(!left.exists() && right.is_dir());
            drop(second);
            assert!(!right.exists());
        }
    }

    #[test]
    fn reraised_group_members_keep_identity_without_retaining_a_cycle() {
        let child = new_exception(Kind::ValueError, Rc::new(Str::from("child")));
        let group = task_group_errors(vec![child.clone(), child.clone()]);
        let weak_group = Rc::downgrade(&group);
        let weak_child = Rc::downgrade(&child);
        // SAFETY: local owners root this exact group and one of its members.
        unsafe {
            let raised = Rc::from_raw(imre_async_member_cause(
                Rc::as_ptr(&child),
                Rc::as_ptr(&group),
            ));
            assert!(Rc::ptr_eq(&child, &raised));
            let first = Rc::from_raw(imre_async_exception_at(Rc::as_ptr(&group), 1));
            assert!(Rc::ptr_eq(&first, &raised));
            drop((first, raised, child));
            assert!(weak_child.upgrade().is_none());
            let first = Rc::from_raw(imre_async_exception_at(Rc::as_ptr(&group), 1));
            let second = Rc::from_raw(imre_async_exception_at(Rc::as_ptr(&group), 3));
            assert!(Rc::ptr_eq(&first, &second));
            let cause = Rc::from_raw(imre_exception_cause(Rc::as_ptr(&second)));
            assert!(Rc::ptr_eq(&cause, &group));
            drop((first, second, cause, group));
            assert!(weak_group.upgrade().is_none());
        }
    }

    fn take_error(kind: Kind) -> Rc<Object> {
        assert_eq!(imre_error_pending(), 1);
        let value = imre_error_take();
        assert!(!value.is_null());
        assert_eq!(imre_error_pending(), 0);
        // SAFETY: take transfers a non-null owned exception reference.
        let value = unsafe { Rc::from_raw(value) };
        // SAFETY: the owned exception is rooted and the expected kind is valid.
        let matches = unsafe { imre_exception_matches(Rc::as_ptr(&value), kind as u32) };
        assert_eq!(matches, 1);
        value
    }

    fn message(value: &Rc<Object>) -> Rc<Str> {
        // SAFETY: the rooted exception accessor transfers one string reference.
        unsafe { Rc::from_raw(imre_exception_message(Rc::as_ptr(value))) }
    }

    #[test]
    fn key_error_owns_its_key_until_the_exception_is_released() {
        let key = Rc::new(Str::new("a quoted 'key'\n"));
        let expected = string_repr(&key);
        let weak = Rc::downgrade(&key);
        // SAFETY: the key is rooted and the constructor returns one owned exception.
        let error =
            unsafe { Rc::from_raw(imre_exception_new(Kind::KeyError as u32, Rc::as_ptr(&key))) };
        drop(key);
        assert_eq!(weak.strong_count(), 1);
        assert_eq!(*message(&error), expected);
        drop(error);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn explicit_causes_preserve_identity_and_independent_ownership() {
        let original = new_exception(Kind::KeyError, Rc::new(Str::new("original")));
        let wrapped = new_exception(Kind::ValueError, Rc::new(Str::new("wrapped")));
        let weak = Rc::downgrade(&original);
        // SAFETY: wrapped is fresh; every borrowed input remains rooted and each
        // transferred reference is adopted immediately.
        unsafe {
            assert!(imre_exception_cause(Rc::as_ptr(&wrapped)).is_null());
            assert_eq!(imre_exception_suppress_context(Rc::as_ptr(&wrapped)), 0);
            let result = Rc::from_raw(imre_exception_with_cause(
                Rc::as_ptr(&wrapped),
                Rc::as_ptr(&original),
            ));
            assert!(Rc::ptr_eq(&result, &wrapped));
            assert_eq!(imre_exception_suppress_context(Rc::as_ptr(&wrapped)), 1);
            let cause = Rc::from_raw(imre_exception_cause(Rc::as_ptr(&wrapped)));
            assert!(Rc::ptr_eq(&cause, &original));
            drop((original, wrapped, result));
            assert_eq!(message(&cause).as_str(), "original");
            drop(cause);
            assert!(weak.upgrade().is_none());

            let suppressed = new_exception(Kind::ValueError, Rc::new(Str::new("hidden")));
            drop(Rc::from_raw(imre_exception_with_cause(
                Rc::as_ptr(&suppressed),
                std::ptr::null(),
            )));
            assert!(imre_exception_cause(Rc::as_ptr(&suppressed)).is_null());
            assert_eq!(imre_exception_suppress_context(Rc::as_ptr(&suppressed)), 1);
        }
    }

    #[test]
    fn deep_cause_chains_release_without_recursive_drop() {
        let message = Rc::new(Str::new("chain"));
        let mut tail = new_exception(Kind::ValueError, message.clone());
        let weak_first = Rc::downgrade(&tail);
        for _ in 0..100_000 {
            let next = new_exception(Kind::ValueError, message.clone());
            // SAFETY: next is fresh and tail remains rooted until attachment.
            unsafe {
                drop(Rc::from_raw(imre_exception_with_cause(
                    Rc::as_ptr(&next),
                    Rc::as_ptr(&tail),
                )));
            }
            tail = next;
        }
        drop(tail);
        assert!(weak_first.upgrade().is_none());
        assert_eq!(Rc::strong_count(&message), 1);
    }

    #[test]
    fn pending_and_active_errors_transfer_independent_owned_references() {
        let text = Rc::new(Str::new("outer"));
        let weak_text = Rc::downgrade(&text);
        // SAFETY: the argument is a rooted string; construction transfers an owner.
        let outer = unsafe {
            Rc::from_raw(imre_exception_new(
                Kind::ValueError as u32,
                Rc::as_ptr(&text),
            ))
        };
        let weak_outer = Rc::downgrade(&outer);
        drop(text);
        // SAFETY: all exception inputs are rooted and every transferred owner is
        // adopted or released below, including same-value pending replacement.
        unsafe {
            imre_exception_raise(Rc::as_ptr(&outer));
            imre_error_set(Rc::as_ptr(&outer));
            let pending = take_error(Kind::ValueError);
            assert!(Rc::ptr_eq(&outer, &pending));
            assert!(imre_exception_swap_active(Rc::as_ptr(&outer)).is_null());
            drop(pending);
            let inner = Rc::from_raw(imre_exception_new(
                Kind::IndexError as u32,
                std::ptr::null(),
            ));
            let previous = Rc::from_raw(imre_exception_swap_active(Rc::as_ptr(&inner)));
            assert!(Rc::ptr_eq(&outer, &previous));
            drop(outer);
            let current = Rc::from_raw(imre_exception_current());
            assert!(Rc::ptr_eq(&inner, &current));
            imre_exception_reraise();
            let raised = take_error(Kind::IndexError);
            assert!(Rc::ptr_eq(&inner, &raised));
            let displaced = Rc::from_raw(imre_exception_swap_active(Rc::as_ptr(&previous)));
            assert!(Rc::ptr_eq(&inner, &displaced));
            drop((inner, current, raised, displaced, previous));
            imre_exception_reraise();
            let raised = take_error(Kind::ValueError);
            assert_eq!(message(&raised).as_str(), "outer");
            drop(raised);
            imre_exception_release(imre_exception_swap_active(std::ptr::null()));
        }
        assert!(weak_outer.upgrade().is_none());
        assert!(weak_text.upgrade().is_none());
        assert!(imre_exception_current().is_null());
        imre_exception_reraise();
        assert_eq!(
            message(&take_error(Kind::RuntimeError)).as_str(),
            "No active exception to reraise"
        );
    }

    #[test]
    fn replacing_clearing_and_reporting_pending_errors_allow_later_recovery() {
        let first = new_exception(Kind::ValueError, Rc::new(Str::new("first")));
        let second = new_exception(Kind::IndexError, Rc::new(Str::new("second")));
        let weak_first = Rc::downgrade(&first);
        let weak_second = Rc::downgrade(&second);
        // SAFETY: inputs stay rooted through acquisition, then pending state
        // becomes their sole owner. Null explicitly clears that owner.
        unsafe {
            imre_error_set(Rc::as_ptr(&first));
            drop(first);
            assert_eq!(imre_error_pending(), 1);
            imre_error_set(Rc::as_ptr(&second));
            drop(second);
            assert!(weak_first.upgrade().is_none());
            assert_eq!(imre_error_pending(), 1);
            imre_error_set(std::ptr::null());
        }
        assert!(weak_second.upgrade().is_none());
        assert_eq!(imre_error_pending(), 0);
        assert!(imre_error_take().is_null());

        let reported = new_exception(Kind::RuntimeError, Rc::new(Str::new("reported test error")));
        let weak_reported = Rc::downgrade(&reported);
        // SAFETY: pending and active independently acquire this rooted exception.
        unsafe {
            imre_error_set(Rc::as_ptr(&reported));
            assert!(imre_exception_swap_active(Rc::as_ptr(&reported)).is_null());
        }
        drop(reported);
        assert_eq!(imre_error_unhandled(), 1);
        assert!(weak_reported.upgrade().is_none());
        assert_eq!(imre_error_pending(), 0);
        assert!(imre_exception_current().is_null());
        assert_eq!(imre_error_unhandled(), 0);
        for _ in 0..2 {
            imre_exception_reraise();
            assert_eq!(
                message(&take_error(Kind::RuntimeError)).as_str(),
                "No active exception to reraise"
            );
        }
    }

    #[test]
    fn builtin_hierarchy_and_keyerror_string_arguments_match_python() {
        for (actual, expected, matches) in [
            (Kind::ZeroDivisionError, Kind::ArithmeticError, true),
            (Kind::OverflowError, Kind::ArithmeticError, true),
            (Kind::IndexError, Kind::LookupError, true),
            (Kind::KeyError, Kind::LookupError, true),
            (Kind::ValueError, Kind::ArithmeticError, false),
            (Kind::SystemError, Kind::Exception, true),
            (Kind::Exception, Kind::BaseException, true),
            (Kind::BaseException, Kind::Exception, false),
        ] {
            // SAFETY: valid kinds and a null zero-argument message satisfy the ABI.
            unsafe {
                let value = Rc::from_raw(imre_exception_new(actual as u32, std::ptr::null()));
                assert_eq!(
                    imre_exception_matches(Rc::as_ptr(&value), expected as u32),
                    u32::from(matches)
                );
            }
        }
        for (argument, expected) in [
            (None, ""),
            (Some(""), "''"),
            (Some("a'b"), "\"a'b\""),
            (Some("a'\"b"), "'a\\'\"b'"),
            (Some("\n\t\r\\\0"), "'\\n\\t\\r\\\\\\x00'"),
            (
                Some("é e\u{301} 😀\u{a0}\u{2028}\u{e000}"),
                "'é e\u{301} 😀\\xa0\\u2028\\ue000'",
            ),
        ] {
            let argument = argument.map(|value| Rc::new(Str::new(value)));
            let pointer = argument.as_ref().map_or(std::ptr::null(), Rc::as_ptr);
            // SAFETY: the nullable argument remains rooted throughout construction.
            let error = unsafe { Rc::from_raw(imre_exception_new(Kind::KeyError as u32, pointer)) };
            drop(argument);
            assert_eq!(message(&error).as_str(), expected);
        }
    }

    #[test]
    fn caught_arithmetic_and_list_failures_preserve_owners_and_allow_recovery() {
        let big = Rc::new(BigInt::from(1_u8) << 120_u32);
        let weak = Rc::downgrade(&big);
        let integer = Int(Rc::into_raw(big) as usize);
        // SAFETY: the constructor transfers one owned list reference.
        let list = unsafe { Rc::from_raw(imre_list_new()) };
        let zero = Int::small(0).unwrap();
        // SAFETY: all integer/list inputs stay rooted, and each successful owned
        // result is adopted exactly once. Failure sentinels are never acquired.
        unsafe {
            assert_eq!(imre_int_binary(integer.0, zero.0, 3), 1);
            drop(take_error(Kind::ZeroDivisionError));
            assert_eq!(imre_int_parse(&Str::new("12__3")), 1);
            drop(take_error(Kind::ValueError));
            imre_list_append(Rc::as_ptr(&list), integer.0);
            drop(integer);
            assert_eq!(weak.strong_count(), 1);
            imre_list_copy_index(Rc::as_ptr(&list), 1, 0);
            drop(take_error(Kind::IndexError));
            assert_eq!(weak.strong_count(), 1);
            assert_eq!(imre_list_get_index(Rc::as_ptr(&list), 7), 0);
            drop(take_error(Kind::IndexError));
            let huge = Int::new(BigInt::from(1_u8) << 100_u32);
            assert!(imre_list_repeat(Rc::as_ptr(&list), huge.0).is_null());
            drop(take_error(Kind::OverflowError));
            let capacity = Int::new(BigInt::from(isize::MAX));
            assert!(imre_list_repeat(Rc::as_ptr(&list), capacity.0).is_null());
            drop(take_error(Kind::MemoryError));
            let popped = Int(imre_list_pop(Rc::as_ptr(&list)));
            assert_eq!(popped.big().as_ref(), &(BigInt::from(1_u8) << 120_u32));
            drop(popped);
            assert_eq!(imre_list_pop(Rc::as_ptr(&list)), 0);
            drop(take_error(Kind::IndexError));
        }
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn failed_sequence_updates_release_staged_values_after_access() {
        let text = Rc::new(Str::new("staged"));
        let weak = Rc::downgrade(&text);
        // SAFETY: constructors transfer owners; all string/exception slots use
        // their declared representation and every getter's owner is adopted.
        unsafe {
            let sequence = Rc::from_raw(imre_seq_new(3));
            imre_seq_set_index(Rc::as_ptr(&sequence), 0, Rc::as_ptr(&text) as usize, 3);
            drop(take_error(Kind::IndexError));
            assert_eq!(Rc::strong_count(&text), 1);
            assert_eq!(imre_seq_pop(Rc::as_ptr(&sequence)), 0);
            drop(take_error(Kind::IndexError));
            drop(sequence);
            let sequence = Rc::from_raw(imre_seq_new(5));
            let exception = Rc::from_raw(imre_exception_new(
                Kind::Exception as u32,
                Rc::as_ptr(&text),
            ));
            let weak_exception = Rc::downgrade(&exception);
            imre_seq_append(Rc::as_ptr(&sequence), Rc::as_ptr(&exception) as usize, 5);
            drop(exception);
            let exception =
                Rc::from_raw(imre_seq_get_index(Rc::as_ptr(&sequence), 0) as *const Object);
            imre_exception_raise(Rc::as_ptr(&exception));
            drop((sequence, exception, text));
            let pending = take_error(Kind::Exception);
            assert_eq!(message(&pending).as_str(), "staged");
            drop(pending);
            assert!(weak_exception.upgrade().is_none());
        }
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn mapping_errors_preserve_outputs_and_scalar_key_messages() {
        let string_key = Rc::new(Str::new("a'b\n"));
        let integer_key = Int::new(BigInt::from(1_u8) << 100_u32);
        // SAFETY: dictionary keys/values match their declared representations;
        // iterator output words are independent writable stack slots.
        unsafe {
            let strings = Rc::from_raw(imre_dict_new(3, 2));
            assert_eq!(
                imre_dict_get(Rc::as_ptr(&strings), Rc::as_ptr(&string_key) as usize),
                0
            );
            assert_eq!(message(&take_error(Kind::KeyError)).as_str(), "\"a'b\\n\"");
            imre_dict_delete(Rc::as_ptr(&strings), Rc::as_ptr(&string_key) as usize);
            assert_eq!(message(&take_error(Kind::KeyError)).as_str(), "\"a'b\\n\"");
            let integers = Rc::from_raw(imre_dict_new(2, 2));
            assert_eq!(imre_dict_get(Rc::as_ptr(&integers), integer_key.0), 0);
            assert_eq!(
                message(&take_error(Kind::KeyError)).as_str(),
                "1267650600228229401496703205376"
            );
            imre_dict_set(Rc::as_ptr(&integers), integer_key.0, 1);
            let iterator = Rc::from_raw(imre_dict_iter(Rc::as_ptr(&integers), 1));
            imre_dict_delete(Rc::as_ptr(&integers), integer_key.0);
            let (mut key, mut value) = (17, 19);
            assert_eq!(
                imre_mapping_iter_next(Rc::as_ptr(&iterator), &mut key, &mut value),
                0
            );
            drop(take_error(Kind::RuntimeError));
            assert_eq!((key, value), (17, 19));
            imre_dict_set(Rc::as_ptr(&integers), integer_key.0, 1);
            assert_eq!(
                imre_mapping_iter_next(Rc::as_ptr(&iterator), &mut key, &mut value),
                0
            );
            drop(take_error(Kind::RuntimeError));
            assert_eq!((key, value), (17, 19));
        }
    }

    #[test]
    fn standalone_string_repr_uses_the_escaped_size() {
        let value = "\0".repeat(16_384);
        let rendered = string_repr(&value);
        assert_eq!(rendered.len(), 65_538);
        assert!(rendered.starts_with("'\\x00"));
        assert!(rendered.ends_with("\\x00'"));
        assert!(rendered.capacity() < 128 * 1024);
    }
}
