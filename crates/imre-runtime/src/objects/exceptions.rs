//! Explicit language errors; no Rust unwind crosses the generated-code ABI.
//!
//! Pending and dynamically active errors own independent references. Their
//! Explicit causes form an immutable chain: the compiler attaches them only to
//! freshly constructed, unescaped exceptions. No traceback or mutable exception
//! attributes can introduce ownership cycles.

use crate::StrOwner;
mod arguments;
mod imports;
pub(super) use imports::imre_exception_field_repr;
mod tree;
pub(super) use tree::append_tree_entries;

use std::cell::{Cell, RefCell};
use std::io::{self, Write};
use std::rc::Rc;

use num_traits::ToPrimitive;
use ruff_python_literal::escape::{Escape, UnicodeEscape};

use allocator_api2::boxed::Box;

use super::words::{OwnedWord, WordKind};
use super::{Object, Storage, StorageCell, Str, fail, object};

mod os;
pub(super) use os::Filename;

#[derive(Clone, Debug)]
pub(crate) struct OsErrorArgs {
    pub(crate) errno: i64,
    pub(crate) description: String,
    /// Both path fields are available when the native exception contains text paths or no paths.
    pub(crate) text_paths: Option<[Option<String>; 2]>,
}

#[repr(C)]
pub struct NamedDescriptor {
    name: *const u8,
    length: usize,
    kinds: *const u32,
    count: usize,
    base: u32,
}

impl NamedDescriptor {
    fn mro(&self) -> &[u32] {
        // SAFETY: the compiler emits count entries in C3 order in static storage.
        unsafe { std::slice::from_raw_parts(self.kinds, self.count) }
    }
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
    // Module-specific OSError subclasses have stable, separately assigned ABI IDs.
    ShutilError = 64,
    SameFileError = 65,
    SpecialFileError = 66,
    CsvError = 67,
    GaiError = 68,
    BinasciiError = 69,
    BinasciiIncomplete = 70,
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
    UnicodeEncodeError = 38,
    NameError = 39,
    UnboundLocalError = 40,
    ImportError = 41,
    ModuleNotFoundError = 42,
    RecursionError = 43,
    BlockingIOError = 44,
    ConnectionError = 45,
    BrokenPipeError = 46,
    ConnectionAbortedError = 47,
    ConnectionRefusedError = 48,
    ConnectionResetError = 49,
    InterruptedError = 50,
    TimeoutError = 51,
    // Warning exception identities use a separate block from module errors and filter categories.
    UnidiffParseError = 52,
    ReferenceError = 53,
    BufferError = 54,
    Warning = 80,
    RuntimeWarning = 81,
    DeprecationWarning = 82,
}

impl Kind {
    fn from_raw(kind: u32) -> Self {
        Self::try_from_raw(kind).unwrap_or_else(|| fail("SystemError", "invalid exception kind"))
    }

    fn try_from_raw(kind: u32) -> Option<Self> {
        Some(match kind {
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
            64 => Self::ShutilError,
            65 => Self::SameFileError,
            66 => Self::SpecialFileError,
            67 => Self::CsvError,
            68 => Self::GaiError,
            69 => Self::BinasciiError,
            70 => Self::BinasciiIncomplete,
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
            38 => Self::UnicodeEncodeError,
            39 => Self::NameError,
            40 => Self::UnboundLocalError,
            41 => Self::ImportError,
            42 => Self::ModuleNotFoundError,
            43 => Self::RecursionError,
            44 => Self::BlockingIOError,
            45 => Self::ConnectionError,
            46 => Self::BrokenPipeError,
            47 => Self::ConnectionAbortedError,
            48 => Self::ConnectionRefusedError,
            49 => Self::ConnectionResetError,
            50 => Self::InterruptedError,
            51 => Self::TimeoutError,
            52 => Self::UnidiffParseError,
            53 => Self::ReferenceError,
            54 => Self::BufferError,
            80 => Self::Warning,
            81 => Self::RuntimeWarning,
            82 => Self::DeprecationWarning,
            _ => return None,
        })
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
            Self::ReferenceError => "ReferenceError",
            Self::BufferError => "BufferError",
            Self::EOFError => "EOFError",
            Self::OSError => "OSError",
            Self::ShutilError => "Error",
            Self::SameFileError => "SameFileError",
            Self::SpecialFileError => "SpecialFileError",
            Self::TypeError => "TypeError",
            Self::AttributeError => "AttributeError",
            Self::MemoryError => "MemoryError",
            Self::SystemError => "SystemError",
            Self::UnicodeError => "UnicodeError",
            Self::UnicodeDecodeError => "UnicodeDecodeError",
            Self::UnicodeEncodeError => "UnicodeEncodeError",
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
            Self::CsvError => "_csv.Error",
            Self::BinasciiError => "binascii.Error",
            Self::BinasciiIncomplete => "binascii.Incomplete",
            Self::GaiError => "socket.gaierror",
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
            Self::ImportError => "ImportError",
            Self::ModuleNotFoundError => "ModuleNotFoundError",
            Self::RecursionError => "RecursionError",
            Self::BlockingIOError => "BlockingIOError",
            Self::ConnectionError => "ConnectionError",
            Self::BrokenPipeError => "BrokenPipeError",
            Self::ConnectionAbortedError => "ConnectionAbortedError",
            Self::ConnectionRefusedError => "ConnectionRefusedError",
            Self::ConnectionResetError => "ConnectionResetError",
            Self::InterruptedError => "InterruptedError",
            Self::TimeoutError => "TimeoutError",
            Self::Warning => "Warning",
            Self::RuntimeWarning => "RuntimeWarning",
            Self::DeprecationWarning => "DeprecationWarning",
            Self::UnidiffParseError => "UnidiffParseError",
        }
    }

    fn matches(self, expected: Self) -> bool {
        match expected {
            Self::BaseException => true,
            Self::Warning => matches!(
                self,
                Self::Warning | Self::RuntimeWarning | Self::DeprecationWarning
            ),
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
                    | Self::BinasciiError
                    | Self::UnicodeError
                    | Self::UnicodeDecodeError
                    | Self::UnicodeEncodeError
                    | Self::UnsupportedOperation
            ),
            Self::UnicodeError => matches!(
                self,
                Self::UnicodeError | Self::UnicodeDecodeError | Self::UnicodeEncodeError
            ),
            Self::ImportError => matches!(self, Self::ImportError | Self::ModuleNotFoundError),
            Self::SubprocessError => {
                matches!(
                    self,
                    Self::SubprocessError | Self::CalledProcessError | Self::TimeoutExpired
                )
            }
            Self::RuntimeError => matches!(
                self,
                Self::RuntimeError | Self::NotImplementedError | Self::RecursionError
            ),
            Self::ShutilError => matches!(self, Self::ShutilError | Self::SameFileError),
            Self::ConnectionError => matches!(
                self,
                Self::ConnectionError
                    | Self::BrokenPipeError
                    | Self::ConnectionAbortedError
                    | Self::ConnectionRefusedError
                    | Self::ConnectionResetError
            ),
            Self::OSError => matches!(
                self,
                Self::OSError
                    | Self::UnsupportedOperation
                    | Self::FileNotFoundError
                    | Self::BlockingIOError
                    | Self::PermissionError
                    | Self::IsADirectoryError
                    | Self::NotADirectoryError
                    | Self::FileExistsError
                    | Self::ConnectionError
                    | Self::BrokenPipeError
                    | Self::ConnectionAbortedError
                    | Self::ConnectionRefusedError
                    | Self::ConnectionResetError
                    | Self::InterruptedError
                    | Self::TimeoutError
                    | Self::GaiError
                    | Self::BadGzipFile
                    | Self::ShutilError
                    | Self::SameFileError
                    | Self::SpecialFileError
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
    details: Option<crate::Shared<ExceptionDetails>>,
}

#[derive(Clone, Default)]
struct ExceptionDetails {
    cause: Option<crate::ObjectOwner>,
    display_context: Option<(crate::ObjectOwner, bool)>,
    origins: Vec<(StrOwner, u64)>,
    named: Option<&'static NamedDescriptor>,
    layout: Option<Rc<NamedPayload>>,
    directories: Option<Rc<DirectoryFrame>>,
    args: Option<arguments::Cached>,
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
    managers: Vec<crate::ObjectOwner>,
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
    Message(StrOwner),
    /// Some builtins render a human message that differs from their original arguments.
    Arguments(Box<BuiltinArguments>),
    Json(Rc<JsonError>),
    Tree(Rc<tree::Errors>),
    Key(OwnedWord),
    Exit(Option<OwnedWord>),
    /// With multiple positional inputs the exit code is the argument tuple itself.
    ExitArguments(imports::Positional),
    Import(Box<imports::Import>),
    Positional(imports::Positional),
    Process(Box<super::processes::ProcessError>),
    Timeout(Box<super::processes::TimeoutError>),
    Group(std::boxed::Box<[GroupMember]>),
}

/// Indirect so retaining several arguments does not grow every Object allocation.
#[derive(Clone)]
struct BuiltinArguments {
    message: StrOwner,
    args: smallvec::SmallVec<[OwnedWord; 5]>,
    os_paths: Option<Box<os::Filenames>>,
}

struct JsonError {
    msg: StrOwner,
    doc: StrOwner,
    pos: usize,
    lineno: usize,
    colno: usize,
}

impl From<StrOwner> for Payload {
    fn from(message: StrOwner) -> Self {
        Self::Message(message)
    }
}

/// A member raised explicitly from its owning group needs both observable
/// directions without an Rc cycle. A weak identity is enough while anyone
/// holds the child. If only the group survives, its immutable value can be
/// materialized again; simultaneous projections preserve object identity.
#[derive(Clone)]
enum GroupMember {
    Strong(crate::ObjectOwner),
    Caused(Rc<CausedMember>),
}

struct CausedMember {
    live: RefCell<crate::WeakObject>,
    value: Exception,
}

impl GroupMember {
    fn get(&self, owner: &crate::ObjectOwner) -> crate::ObjectOwner {
        match self {
            Self::Strong(child) => child.clone(),
            Self::Caused(entry) => {
                if let Some(child) = entry.live.borrow().upgrade() {
                    return child;
                }
                let mut value = entry.value.clone();
                value.set_explicit_cause(Some(owner.clone()));
                let child =
                    crate::ObjectOwner::new(Object(StorageCell::new(Storage::Exception(value))));
                *entry.live.borrow_mut() = crate::ObjectOwner::downgrade(&child);
                child
            }
        }
    }
}

impl Exception {
    /// Own every argument before invoking a formatter that can reinitialize the exception.
    fn display_snapshot(&self) -> Result<Self> {
        fn boxed<T>(value: T) -> Result<Box<T>> {
            Box::try_new(value).map_err(|_| Error::allocation())
        }

        let payload = match &self.payload {
            Payload::Arguments(arguments) => {
                let mut args = smallvec::SmallVec::new();
                args.try_reserve_exact(arguments.args.len())
                    .map_err(|_| Error::allocation())?;
                args.extend(arguments.args.iter().cloned());
                let os_paths = arguments
                    .os_paths
                    .as_deref()
                    .cloned()
                    .map(boxed)
                    .transpose()?;
                Payload::Arguments(boxed(BuiltinArguments {
                    message: arguments.message.clone(),
                    args,
                    os_paths,
                })?)
            }
            Payload::Import(value) => Payload::Import(boxed((**value).clone())?),
            Payload::Process(value) => Payload::Process(boxed((**value).clone())?),
            Payload::Timeout(value) => Payload::Timeout(boxed((**value).clone())?),
            Payload::Group(members) => {
                let mut copy = Vec::new();
                copy.try_reserve_exact(members.len())
                    .map_err(|_| Error::allocation())?;
                copy.extend_from_slice(members);
                Payload::Group(copy.into_boxed_slice())
            }
            Payload::Empty
            | Payload::Message(_)
            | Payload::Json(_)
            | Payload::Tree(_)
            | Payload::Key(_)
            | Payload::Exit(_)
            | Payload::ExitArguments(_)
            | Payload::Positional(_) => self.payload.clone(),
        };
        Ok(Self {
            kind: self.kind,
            payload,
            suppress_context: self.suppress_context,
            details: self.details.clone(),
        })
    }

    fn attach_directory_frame(&mut self, mut details: crate::Shared<ExceptionDetails>) {
        if let Some(previous) = self.details.as_deref() {
            let fresh = crate::Shared::get_mut(&mut details).unwrap();
            fresh.cause.clone_from(&previous.cause);
            fresh.display_context.clone_from(&previous.display_context);
            fresh.origins.clone_from(&previous.origins);
            fresh.named = previous.named;
            fresh.layout.clone_from(&previous.layout);
            fresh.args.clone_from(&previous.args);
            Rc::get_mut(fresh.directories.as_mut().unwrap())
                .unwrap()
                .outer = previous.directories.clone();
        }
        self.details = Some(details);
    }

    fn try_details_mut(&mut self) -> Result<&mut ExceptionDetails> {
        if self
            .details
            .as_mut()
            .and_then(crate::Shared::get_mut)
            .is_none()
        {
            let copy = self.details.as_deref().cloned().unwrap_or_default();
            self.details = Some(crate::Shared::try_new(copy).map_err(|_| Error::allocation())?);
        }
        Ok(crate::Shared::get_mut(self.details.as_mut().unwrap()).unwrap())
    }

    fn details_mut(&mut self) -> &mut ExceptionDetails {
        self.try_details_mut().unwrap_or_else(|_| {
            std::alloc::handle_alloc_error(std::alloc::Layout::new::<ExceptionDetails>())
        })
    }

    fn cause(&self) -> Option<&crate::ObjectOwner> {
        self.details.as_deref()?.cause.as_ref()
    }

    fn display_context(&self) -> Option<&(crate::ObjectOwner, bool)> {
        self.details.as_deref()?.display_context.as_ref()
    }

    fn set_explicit_cause(&mut self, cause: Option<crate::ObjectOwner>) {
        if cause.is_some() || self.cause().is_some() || self.display_context().is_some() {
            let details = self.details_mut();
            details.cause = cause;
            details.display_context = None;
        }
        self.suppress_context = true;
    }

    fn take_cause(&mut self) -> Option<crate::ObjectOwner> {
        // A shared snapshot retains its own metadata and keeps the cause rooted.
        let details = crate::Shared::try_unwrap(self.details.take()?).ok()?;
        details
            .cause
            .or_else(|| details.display_context.map(|(parent, _)| parent))
    }

    /// KeyError uses repr for one key unless an earlier builtin in the MRO overrides str.
    fn quotes_single_key(&self) -> bool {
        let Some(descriptor) = self.named() else {
            return self.kind == Kind::KeyError;
        };
        for &kind in descriptor.mro() {
            if kind == Kind::KeyError as u32 {
                return true;
            }
            if [
                Kind::OSError as u32,
                Kind::ImportError as u32,
                Kind::UnicodeEncodeError as u32,
            ]
            .contains(&kind)
            {
                return false;
            }
        }
        false
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
            let Ok(object) = crate::ObjectOwner::try_unwrap(cause) else {
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
    cause: Option<crate::ObjectOwner>,
}

impl Error {
    pub(super) fn exit_status(status: i32) -> Self {
        let word = crate::Int::small(i64::from(status)).unwrap().into_raw();
        // SAFETY: the tagged integer transfers its owned word to the exception.
        let code = unsafe { OwnedWord::from_owned(word, WordKind::new(2)) };
        Self {
            kind: Kind::SystemExit,
            payload: Some(Payload::Exit(Some(code))),
            cause: None,
        }
    }

    pub(crate) fn new(kind: Kind, message: impl Into<Str>) -> Self {
        Self {
            kind,
            payload: Some(Payload::Message(StrOwner::new(message.into()))),
            cause: None,
        }
    }

    /// Preserve a single object argument while displaying its Python representation.
    pub(super) fn object_argument(kind: Kind, message: String, argument: OwnedWord) -> Self {
        Self {
            kind,
            payload: Some(Payload::Arguments(Box::new(BuiltinArguments {
                message: StrOwner::new(message.into()),
                args: smallvec::smallvec![argument],
                os_paths: None,
            }))),
            cause: None,
        }
    }

    fn string_arg(text: impl Into<Str>) -> OwnedWord {
        let word = StrOwner::into_raw(StrOwner::new(text.into())) as usize;
        // SAFETY: kind 3 owns the transferred string allocation.
        unsafe { OwnedWord::from_owned(word, WordKind::new(3)) }
    }

    fn int_arg(value: crate::Int) -> OwnedWord {
        // SAFETY: kind 2 owns an integer word, whether tagged or allocated.
        unsafe { OwnedWord::from_owned(value.into_raw(), WordKind::new(2)) }
    }

    /// Store errno and strerror in args and retain original path values separately.
    pub(super) fn from_errno(
        kind: Kind,
        errno: i32,
        description: &str,
        filename: Option<Filename>,
        filename2: Option<Filename>,
    ) -> Self {
        let message = format!("[Errno {errno}] {description}");
        let os_paths = filename.map(|first| {
            Box::new(os::Filenames {
                first,
                second: filename2,
            })
        });
        Self::errno_arguments(kind, errno, description, message, os_paths, None)
    }

    pub(super) fn blocked_write(errno: i32, accepted: usize) -> Self {
        let description = crate::BlockedWrite::DESCRIPTION;
        Self::errno_arguments(
            Kind::BlockingIOError,
            errno,
            description,
            format!("[Errno {errno}] {description}"),
            None,
            Some(accepted),
        )
    }

    fn errno_arguments(
        kind: Kind,
        errno: i32,
        description: &str,
        message: String,
        os_paths: Option<Box<os::Filenames>>,
        accepted: Option<usize>,
    ) -> Self {
        let mut args = smallvec::smallvec![
            Self::int_arg(crate::Int::small(i64::from(errno)).unwrap()),
            Self::string_arg(description),
        ];
        if let Some(count) = accepted {
            args.push(Self::int_arg(crate::Int::from_usize(count)));
        }
        Self {
            kind,
            payload: Some(Payload::Arguments(Box::new(BuiltinArguments {
                message: StrOwner::new(message.into()),
                args,
                os_paths,
            }))),
            cause: None,
        }
    }

    /// Codecs expose the original object and half-open character or byte interval in args.
    fn unicode_codec(
        kind: Kind,
        codec: &str,
        object: OwnedWord,
        bounds: std::ops::Range<usize>,
        reason: &str,
        message: String,
    ) -> Self {
        let args = smallvec::smallvec![
            Self::string_arg(codec),
            object,
            Self::int_arg(crate::Int::from_usize(bounds.start)),
            Self::int_arg(crate::Int::from_usize(bounds.end)),
            Self::string_arg(reason),
        ];
        Self {
            kind,
            payload: Some(Payload::Arguments(Box::new(BuiltinArguments {
                message: StrOwner::new(message.into()),
                args,
                os_paths: None,
            }))),
            cause: None,
        }
    }

    pub(super) fn unicode_decode(
        codec: &str,
        bytes: crate::BytesOwner,
        bounds: std::ops::Range<usize>,
        reason: &str,
        message: String,
    ) -> Self {
        // SAFETY: kind 8 owns the immutable input of the failed decode.
        let object = unsafe {
            OwnedWord::from_owned(
                crate::BytesOwner::into_raw(bytes) as usize,
                WordKind::new(8),
            )
        };
        Self::unicode_codec(
            Kind::UnicodeDecodeError,
            codec,
            object,
            bounds,
            reason,
            message,
        )
    }

    pub(super) fn unicode_encode(
        codec: &str,
        text: crate::StrOwner,
        bounds: std::ops::Range<usize>,
        reason: &str,
        message: String,
    ) -> Self {
        // SAFETY: kind 3 owns the Unicode input of the failed encode.
        let object =
            unsafe { OwnedWord::from_owned(StrOwner::into_raw(text) as usize, WordKind::new(3)) };
        Self::unicode_codec(
            Kind::UnicodeEncodeError,
            codec,
            object,
            bounds,
            reason,
            message,
        )
    }

    pub(super) fn is_kind(&self, kind: Kind) -> bool {
        self.kind == kind
    }

    pub(super) fn json_decode(
        msg: &str,
        doc: StrOwner,
        pos: usize,
        lineno: usize,
        colno: usize,
    ) -> Self {
        Self {
            kind: Kind::JsonDecodeError,
            payload: Some(Payload::Json(Rc::new(JsonError {
                msg: StrOwner::new(Str::from(msg)),
                doc,
                pos,
                lineno,
                colno,
            }))),
            cause: None,
        }
    }

    pub(crate) fn allocation() -> Self {
        Self {
            kind: Kind::MemoryError,
            payload: None,
            cause: None,
        }
    }

    /// Installs a constant diagnostic, preserving a catchable error if either owner cannot grow.
    pub(crate) fn return_static_message<T>(kind: Kind, message: &'static str, sentinel: T) -> T {
        Self::return_checked_message(kind, Str::const_new(message), sentinel)
    }

    pub(crate) fn return_checked_message<T>(kind: Kind, message: Str, sentinel: T) -> T {
        let error = StrOwner::try_new(message).and_then(|message| {
            crate::ObjectOwner::try_new(exception_object(kind, Payload::Message(message), None))
        });
        match error {
            Ok(error) => publish_raised(error),
            Err(_) => Self::memory_error().raise(),
        }
        sentinel
    }

    /// Use Python's argument-free MemoryError when an operation exhausts memory.
    pub(crate) fn memory_error() -> Self {
        Self {
            kind: Kind::MemoryError,
            payload: Some(Payload::Empty),
            cause: None,
        }
    }

    pub(super) fn key(key: OwnedWord) -> Self {
        Self {
            kind: Kind::KeyError,
            payload: Some(Payload::Key(key)),
            cause: None,
        }
    }

    pub(super) fn string_key(key: Str) -> Self {
        let word = StrOwner::into_raw(StrOwner::new(key)) as usize;
        // SAFETY: the new string contributes one owned reference of exact kind 3.
        Self::key(unsafe { OwnedWord::from_owned(word, WordKind::new(3)) })
    }

    pub(crate) fn index() -> Self {
        Self::new(Kind::IndexError, "list index out of range")
    }

    /// Copy the two original OS arguments when transferring a kernel error to boxed Python.
    pub(crate) fn os_args(&self) -> Option<OsErrorArgs> {
        if !self.kind.matches(Kind::OSError) {
            return None;
        }
        let Some(Payload::Arguments(arguments)) = &self.payload else {
            return None;
        };
        let [code, description] = arguments.args.as_slice() else {
            return None;
        };
        if code.kind.raw() != 2 || description.kind.raw() != 3 {
            return None;
        }
        let text_path = |path: Option<&Filename>| match path {
            Some(Filename::Str(path)) => Some(Some(path.to_string())),
            None => Some(None),
            Some(_) => None,
        };
        let paths = arguments.os_paths.as_deref();
        let first = text_path(paths.map(|paths| &paths.first));
        let second = text_path(paths.and_then(|paths| paths.second.as_ref()));
        // SAFETY: both owned argument words have the verified integer and string kinds.
        unsafe {
            Some(OsErrorArgs {
                errno: crate::Int::borrow(code.word).to_i64()?,
                description: (&*(description.word as *const Str)).to_string(),
                text_paths: first.zip(second).map(|(first, second)| [first, second]),
            })
        }
    }

    /// Preserve the native exception class and its formatted Python message.
    pub(crate) fn into_parts(self) -> (&'static str, Rendered) {
        let kind = self.kind.name();
        let error = self.into_object();
        // SAFETY: the local owner keeps the exception payload alive throughout the read.
        let message = unsafe { error.0.with(Storage::exception_message) };
        (kind, message)
    }

    /// Copytree aggregates only OS failures; language errors still escape intact.
    pub(super) fn into_os_message(self) -> Result<StrOwner> {
        if !self.kind.matches(Kind::OSError) {
            return Err(self);
        }
        let error = self.into_object();
        // SAFETY: this fresh exception owns its payload throughout the read.
        unsafe { error.0.with(Storage::exception_message) }.map_err(|_| Error::allocation())
    }

    /// Retain the original exception when a native operation translates an error.
    pub(super) fn with_cause(mut self, cause: Self) -> Self {
        self.cause = Some(cause.into_object());
        self
    }

    pub(crate) fn with_pending_cause(mut self) -> Self {
        self.cause = take_pending();
        self
    }

    fn into_object(self) -> crate::ObjectOwner {
        match self.payload {
            Some(Payload::Empty) if self.kind == Kind::MemoryError && self.cause.is_none() => {
                MEMORY_ERRORS.with(|errors| {
                    let slot = errors[..MEMORY_ERROR_RESERVE]
                        .iter()
                        .find(|error| crate::ObjectOwner::strong_count(&error.borrow()) == 1)
                        .unwrap_or(&errors[MEMORY_ERROR_RESERVE]);
                    slot.borrow().clone()
                })
            }
            Some(payload) => exception_with_cause(self.kind, payload, self.cause),
            None => SEQUENCE_ALLOCATION_ERROR.with_borrow(crate::ObjectOwner::clone),
        }
    }

    pub(super) fn into_exception(self) -> crate::ObjectOwner {
        self.into_object()
    }

    /// Installs this error only after any runtime storage access has ended.
    /// Keep exception allocation and cleanup out of successful access paths.
    #[cold]
    #[inline(never)]
    pub(crate) fn raise(self) {
        // Allocation failures use reserved exception owners and cannot retain a handler.
        if self.kind == Kind::MemoryError {
            replace_pending(Some(self.into_object()));
        } else {
            publish_raised(self.into_object());
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

// CPython reserves sixteen distinct errors and uses one last-resort instance
// while all sixteen are still observable by callers.
const MEMORY_ERROR_RESERVE: usize = 16;

thread_local! {
    static MEMORY_ERRORS: [RefCell<crate::ObjectOwner>; MEMORY_ERROR_RESERVE + 1] =
        std::array::from_fn(|_| RefCell::new(new_exception(Kind::MemoryError, Payload::Empty)));
    static RECORD_ALLOCATION_ERROR: RefCell<crate::ObjectOwner> = RefCell::new(new_exception(
        Kind::MemoryError, StrOwner::new(Str::new("record allocation failed")),
    ));
    static SEQUENCE_ALLOCATION_ERROR: RefCell<crate::ObjectOwner> = RefCell::new(new_exception(
        Kind::MemoryError, StrOwner::new(Str::new("sequence allocation failed")),
    ));
    static PENDING: RefCell<Option<crate::ObjectOwner>> = const { RefCell::new(None) };
    static ACTIVE: RefCell<Option<crate::ObjectOwner>> = const { RefCell::new(None) };
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
    MEMORY_ERRORS.with(|_| ());
    RECORD_ALLOCATION_ERROR.with(|_| ());
    SEQUENCE_ALLOCATION_ERROR.with(|_| ());
    PENDING.with(|_| ());
    ACTIVE.with(|_| ());
}

#[cold]
pub(super) fn record_allocation_failed() {
    RECORD_ALLOCATION_ERROR
        .with_borrow(|error| replace_pending(Some(crate::ObjectOwner::clone(error))));
}

#[cold]
pub(crate) fn sequence_allocation_failed() {
    SEQUENCE_ALLOCATION_ERROR
        .with_borrow(|error| replace_pending(Some(crate::ObjectOwner::clone(error))));
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
    error.details = Some(crate::Shared::new(ExceptionDetails {
        directories: Some(Rc::new(DirectoryFrame {
            managers: Vec::with_capacity(count),
            outer: None,
        })),
        ..ExceptionDetails::default()
    }));
    crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
        Storage::Exception(error),
    ))))
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
                if !roots
                    .iter()
                    .any(|root| crate::ObjectOwner::as_ptr(root) == manager)
                {
                    roots.push(crate::ObjectOwner::acquire_word(manager as usize));
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
    let frame = unsafe { crate::ObjectOwner::from_raw(frame) };
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
        let rotate = |template: &RefCell<crate::ObjectOwner>| {
            if !crate::ObjectOwner::ptr_eq(&template.borrow(), pending) {
                return false;
            }
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
            template.replace(frame.clone());
            true
        };
        if !RECORD_ALLOCATION_ERROR.with(rotate) && !SEQUENCE_ALLOCATION_ERROR.with(rotate) {
            MEMORY_ERRORS.with(|errors| errors.iter().any(rotate));
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

fn new_exception(kind: Kind, payload: impl Into<Payload>) -> crate::ObjectOwner {
    exception_with_cause(kind, payload.into(), None)
}

fn exception_with_cause(
    kind: Kind,
    payload: Payload,
    cause: Option<crate::ObjectOwner>,
) -> crate::ObjectOwner {
    crate::ObjectOwner::new(exception_object(kind, payload, cause))
}

fn exception_object(kind: Kind, payload: Payload, cause: Option<crate::ObjectOwner>) -> Object {
    Object(StorageCell::new(Storage::Exception(Exception {
        kind,
        payload,
        suppress_context: cause.is_some(),
        details: cause.map(|cause| {
            crate::Shared::new(ExceptionDetails {
                cause: Some(cause),
                ..Default::default()
            })
        }),
    })))
}

pub(super) fn kind_of(error: &Object) -> Kind {
    // SAFETY: the task holds an immutable exact exception throughout the read.
    unsafe { error.0.with(Storage::exception_kind) }
}

/// Wrap an escaping StopIteration with the RuntimeError message for its frame kind.
pub(super) fn wrap_stop_iteration(error: crate::ObjectOwner, message: &str) -> crate::ObjectOwner {
    if kind_of(&error) != Kind::StopIteration {
        return error;
    }
    exception_with_cause(
        Kind::RuntimeError,
        Payload::Message(StrOwner::new(message.into())),
        Some(error),
    )
}

/// A generator context manager lets the with-body propagate its original exception,
/// including a StopIteration returned as the cause of a RuntimeError.
pub(super) fn propagates_context_exception(
    raised: &crate::ObjectOwner,
    injected: &crate::ObjectOwner,
) -> bool {
    if crate::ObjectOwner::ptr_eq(raised, injected) {
        return true;
    }
    if kind_of(injected) != Kind::StopIteration {
        return false;
    }
    // SAFETY: both exceptions remain rooted throughout the cause identity check.
    unsafe {
        raised.0.with(|storage| {
            let exception = storage.exception();
            exception.kind.matches(Kind::RuntimeError)
                && exception
                    .cause()
                    .is_some_and(|cause| crate::ObjectOwner::ptr_eq(cause, injected))
        })
    }
}

pub(super) fn task_group_errors(errors: Vec<crate::ObjectOwner>) -> crate::ObjectOwner {
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
    let owner: crate::ObjectOwner = unsafe { crate::ObjectOwner::acquire_word(error as usize) };
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
            Ok(crate::ObjectOwner::into_raw(group[index].get(&owner)))
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
        let child: crate::ObjectOwner = crate::ObjectOwner::acquire_word(child as usize);
        let group: crate::ObjectOwner = crate::ObjectOwner::acquire_word(group as usize);
        let mut snapshot = child.0.with(|storage| storage.exception().clone());
        snapshot.set_explicit_cause(None);
        let entry = Rc::new(CausedMember {
            live: RefCell::new(crate::ObjectOwner::downgrade(&child)),
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
                if matches!(member, GroupMember::Strong(target) if crate::ObjectOwner::ptr_eq(target, &child)) {
                    *member = GroupMember::Caused(entry.clone());
                }
            }
            parent.detach_group_args();
        });
        child.0.with_mut(|storage| {
            let Storage::Exception(value) = storage else {
                unreachable!()
            };
            value.set_explicit_cause(Some(group));
        });
        crate::ObjectOwner::into_raw(child)
    }
}

/// Publishes a completed child's rich failure after all input storage borrows end.
#[cold]
pub(super) fn raise_process(error: super::processes::ProcessError) {
    publish_raised(new_exception(
        Kind::CalledProcessError,
        Payload::Process(Box::new(error)),
    ));
}

/// Publishes the immutable deadline snapshot after the child has been reaped.
#[cold]
pub(super) fn raise_timeout(error: super::processes::TimeoutError) {
    publish_raised(new_exception(
        Kind::TimeoutExpired,
        Payload::Timeout(Box::new(error)),
    ));
}

/// Return the visible parent and whether it was installed as an explicit cause.
fn display_parent(owner: &crate::ObjectOwner) -> Option<(crate::ObjectOwner, bool)> {
    // SAFETY: the owner roots the exception and this borrow cannot call Python.
    unsafe {
        owner.0.with(|s| {
            let e = s.exception();
            e.cause().cloned().map(|cause| (cause, true)).or_else(|| {
                if e.suppress_context {
                    None
                } else {
                    e.details.as_deref()?.display_context.clone()
                }
            })
        })
    }
}

/// A cause or TaskGroup member can already own the escaping exception. Preserve
/// the visible prefix without forming a strong reference cycle through it.
fn detached_display_prefix(
    active: &crate::ObjectOwner,
    escaping: &crate::ObjectOwner,
) -> Result<crate::ObjectOwner> {
    let mut prefix = Vec::new();
    let mut cursor = Some(active.clone());
    while let Some(node) = cursor {
        if crate::ObjectOwner::ptr_eq(&node, escaping) {
            break;
        }
        let parent = display_parent(&node);
        let explicit = parent.as_ref().is_some_and(|(_, explicit)| *explicit);
        prefix.try_reserve(1).map_err(|_| Error::allocation())?;
        prefix.push((node, explicit));
        cursor = parent.map(|(next, _)| next);
    }
    let mut tail = None;
    for (owner, explicit) in prefix.into_iter().rev() {
        // SAFETY: the rooted source stays live while its display payload is copied.
        let mut copy = unsafe {
            owner.0.with(|s| {
                let mut copy = s.exception().display_snapshot()?;
                if matches!(copy.payload, Payload::Group(_)) {
                    // The built-in group message uses its count and does not format children.
                    copy.payload =
                        Payload::Message(s.exception_message().map_err(|_| Error::allocation())?);
                }
                Ok::<_, Error>(copy)
            })
        }?;
        let metadata = copy.details.take();
        let named = metadata.as_deref().and_then(|d| d.named);
        let layout = metadata.as_deref().and_then(|d| d.layout.clone());
        let mut origins = Vec::new();
        if let Some(original) = metadata.as_deref() {
            origins
                .try_reserve_exact(original.origins.len())
                .map_err(|_| Error::allocation())?;
            origins.extend_from_slice(&original.origins);
        }
        if named.is_some() || layout.is_some() || !origins.is_empty() || tail.is_some() {
            copy.details = Some(
                crate::Shared::try_new(ExceptionDetails {
                    named,
                    layout,
                    origins,
                    display_context: tail.map(|parent| (parent, explicit)),
                    ..Default::default()
                })
                .map_err(|_| Error::allocation())?,
            );
        }
        copy.suppress_context = false;
        tail = Some(
            crate::ObjectOwner::try_new(Object(StorageCell::new(Storage::Exception(copy))))
                .map_err(|_| Error::allocation())?,
        );
    }
    Ok(tail.expect("active and escaping are distinct"))
}

/// A group can retain an escaping exception through a nested member or a member's cause.
fn group_owns_exception(group: &crate::ObjectOwner, target: &crate::ObjectOwner) -> Result<bool> {
    let mut pending = Vec::new();
    // SAFETY: the owner roots all members until the temporary owners are acquired.
    unsafe {
        group.0.with(|storage| {
            if let Payload::Group(members) = &storage.exception().payload {
                pending
                    .try_reserve_exact(members.len())
                    .map_err(|_| Error::allocation())?;
                pending.extend_from_slice(members);
            }
            Ok::<_, Error>(())
        })
    }?;
    if pending.is_empty() {
        return Ok(false);
    }
    let mut visited = std::collections::HashSet::new();
    while let Some(member) = pending.pop() {
        let address = match &member {
            GroupMember::Strong(owner) => {
                if crate::ObjectOwner::ptr_eq(owner, target) {
                    return Ok(true);
                }
                crate::ObjectOwner::as_ptr(owner) as usize
            }
            GroupMember::Caused(entry) => Rc::as_ptr(entry) as usize,
        };
        if visited.contains(&address) {
            continue;
        }
        visited.try_reserve(1).map_err(|_| Error::allocation())?;
        visited.insert(address);
        let mut enqueue = |e: &Exception| -> Result<()> {
            let children = match &e.payload {
                Payload::Group(children) => &children[..],
                _ => &[],
            };
            let context = e.display_context().map(|(owner, _)| owner);
            let parents = [e.cause(), context];
            pending
                .try_reserve(children.len() + parents.iter().flatten().count())
                .map_err(|_| Error::allocation())?;
            pending.extend_from_slice(children);
            pending.extend(
                parents
                    .into_iter()
                    .flatten()
                    .cloned()
                    .map(GroupMember::Strong),
            );
            Ok(())
        };
        match member {
            // SAFETY: the owner roots the member while its outgoing edges are copied.
            GroupMember::Strong(owner) => unsafe { owner.0.with(|s| enqueue(s.exception())) }?,
            GroupMember::Caused(entry) => enqueue(&entry.value)?,
        }
    }
    Ok(false)
}

fn attach_display_context(value: &crate::ObjectOwner, active: crate::ObjectOwner) -> Result<()> {
    if crate::ObjectOwner::ptr_eq(value, &active)
        // SAFETY: the caller roots the escaping exception.
        || unsafe { value.0.with(|s| s.exception().suppress_context) }
    {
        return Ok(());
    }
    let mut node = active.clone();
    let mut detach = false;
    loop {
        if !detach {
            detach = group_owns_exception(&node, value)?;
        }
        let Some((next, explicit)) = display_parent(&node) else {
            break;
        };
        if crate::ObjectOwner::ptr_eq(&next, value) {
            if explicit {
                detach = true;
            } else {
                // CPython cuts the preceding implicit edge before reversing this chain.
                // SAFETY: the node is rooted and no other borrow is active.
                unsafe {
                    node.0.with_mut(|s| {
                        let Storage::Exception(e) = s else {
                            unreachable!("exception context")
                        };
                        e.try_details_mut()?.display_context = None;
                        Ok::<_, Error>(())
                    })
                }?;
            }
            break;
        }
        node = next;
    }
    let parent = if detach {
        detached_display_prefix(&active, value)?
    } else {
        active
    };
    // SAFETY: both owners remain rooted while the new edge is installed.
    unsafe {
        value.0.with_mut(|s| {
            let Storage::Exception(e) = s else {
                unreachable!("exception context")
            };
            e.try_details_mut()?.display_context = Some((parent, false));
            Ok(())
        })
    }
}

fn publish_raised(value: crate::ObjectOwner) {
    if let Some(active) = active_exception()
        && let Err(error) = attach_display_context(&value, active)
    {
        error.raise();
        return;
    }
    replace_pending(Some(value));
}

/// Replaces the pending owner without dropping it during a TLS mutable borrow.
pub(super) fn replace_pending(value: Option<crate::ObjectOwner>) {
    let present = value.is_some();
    let previous = PENDING.with(|pending| pending.replace(value));
    PENDING_PRESENT.set(present);
    drop(previous);
}

/// Clears presence before transferring the owner, without dropping under a TLS borrow.
pub(super) fn take_pending() -> Option<crate::ObjectOwner> {
    let pending = PENDING.with(RefCell::take);
    PENDING_PRESENT.set(false);
    pending
}

/// A suspended task retains the exception dynamically active in its own handlers.
pub(super) fn swap_active(saved: &mut Option<crate::ObjectOwner>) {
    ACTIVE.with(|active| std::mem::swap(&mut *active.borrow_mut(), saved));
}

pub(super) fn active_exception() -> Option<crate::ObjectOwner> {
    ACTIVE.with(|active| active.borrow().clone())
}

/// Quote and escape a string with CPython 3.14's Unicode 16 printability rules.
pub(crate) fn string_repr(value: &str) -> Str {
    let mut result = String::new();
    push_string_repr(value, &mut result);
    Str::from(result)
}

pub(super) fn push_string_repr(value: &str, out: &mut String) {
    write_quoted_repr(value, out).expect("writing to a String cannot fail");
}

pub(super) fn write_quoted_repr(
    value: &str,
    out: &mut impl super::values::ReprTarget,
) -> std::fmt::Result {
    // These printable ASCII strings use single quotes and have no escapes.
    if value
        .bytes()
        .all(|byte| (b' '..=b'~').contains(&byte) && !matches!(byte, b'\'' | b'\\'))
    {
        let size = value.len().checked_add(2).ok_or(std::fmt::Error)?;
        let buffer = out.quoted_buffer(size)?;
        buffer.push('\'');
        buffer.push_str(value);
        buffer.push('\'');
        return Ok(());
    }
    let (escaped, correct_printability, size) = quoted_layout(value)?;
    write_escaped_string(&escaped, out.quoted_buffer(size)?, correct_printability)
}

fn quoted_layout(
    value: &str,
) -> std::result::Result<(UnicodeEscape<'_>, bool, usize), std::fmt::Error> {
    let escaped = UnicodeEscape::new_repr(value);
    let extra = if value.is_ascii() {
        0
    } else {
        value
            .chars()
            .filter(|&ch| crate::integer_text::newer_printable(ch))
            .fold(0usize, |extra, ch| {
                let width = if ch <= '\u{ffff}' { 6 } else { 10 };
                extra.saturating_add(width - ch.len_utf8())
            })
    };
    let size = escaped
        .layout()
        .len
        .and_then(|body| body.checked_add(2)?.checked_add(extra))
        .ok_or(std::fmt::Error)?;
    Ok((escaped, extra != 0, size))
}

pub(crate) fn write_string_repr(value: &str, out: &mut impl std::fmt::Write) -> std::fmt::Result {
    write_escaped_string(&UnicodeEscape::new_repr(value), out, !value.is_ascii())
}

/// Render the portable spelling into a container repr without first copying the whole path.
/// A literal backslash is the only character whose quoted spelling starts with two backslashes.
pub(super) fn write_windows_path_repr(
    value: &str,
    out: &mut impl super::values::ReprTarget,
) -> std::fmt::Result {
    struct PosixSeparators<'a> {
        out: &'a mut String,
        escape: bool,
    }

    impl std::fmt::Write for PosixSeparators<'_> {
        fn write_char(&mut self, ch: char) -> std::fmt::Result {
            if self.escape {
                if ch == '\\' {
                    self.out.push('/');
                } else {
                    self.out.push('\\');
                    self.out.push(ch);
                }
                self.escape = false;
            } else if ch == '\\' {
                self.escape = true;
            } else {
                self.out.push(ch);
            }
            Ok(())
        }

        fn write_str(&mut self, text: &str) -> std::fmt::Result {
            for ch in text.chars() {
                self.write_char(ch)?;
            }
            Ok(())
        }
    }

    let (escaped, correct_printability, size) = quoted_layout(value)?;
    // Each quoted backslash is replaced by a single slash.
    let size = size - value.bytes().filter(|&byte| byte == b'\\').count();
    let mut writer = PosixSeparators {
        out: out.quoted_buffer(size)?,
        escape: false,
    };
    write_escaped_string(&escaped, &mut writer, correct_printability)?;
    debug_assert!(!writer.escape);
    Ok(())
}

fn write_escaped_string(
    escaped: &UnicodeEscape<'_>,
    out: &mut impl std::fmt::Write,
    correct_printability: bool,
) -> std::fmt::Result {
    let quote = escaped.layout().quote.as_char();
    out.write_char(quote)?;
    if correct_printability {
        escaped.write_body(&mut Unicode16Repr(out))?;
    } else {
        escaped.write_body(out)?;
    }
    out.write_char(quote)
}

/// Stream Ruff's escaped body, escaping characters first assigned after Unicode 16.
struct Unicode16Repr<'a, W>(&'a mut W);

impl<W: std::fmt::Write> std::fmt::Write for Unicode16Repr<'_, W> {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        let mut start = 0;
        if !text.is_ascii() {
            for (i, ch) in text
                .char_indices()
                .filter(|&(_, ch)| crate::integer_text::newer_printable(ch))
            {
                self.0.write_str(&text[start..i])?;
                let code = u32::from(ch);
                if code <= 0xffff {
                    write!(self.0, "\\u{code:04x}")?;
                } else {
                    write!(self.0, "\\U{code:08x}")?;
                }
                start = i + ch.len_utf8();
            }
        }
        self.0.write_str(&text[start..])
    }
}

/// Acquires nullable exception input without retaining a storage reference.
unsafe fn acquire_exception(value: *const Object) -> Option<crate::ObjectOwner> {
    if value.is_null() {
        return None;
    }
    // SAFETY: the caller roots the object; only immutable kind metadata escapes.
    unsafe { object(value).0.with(Storage::exception_kind) };
    // SAFETY: the shape-checked handle remains rooted throughout acquisition.
    Some(unsafe { crate::ObjectOwner::acquire_word(value as usize) })
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

    fn exception_message(&self) -> Rendered {
        use std::fmt::Write as _;
        match &self.exception().payload {
            Payload::Import(err) => err.message(),
            Payload::Positional(args) => args.message(self.exception().quotes_single_key()),
            Payload::ExitArguments(args) => args.message(false),
            Payload::Exit(Some(code)) => {
                super::boxed_values::word_text(code.kind, code.word, false)
            }
            Payload::Exit(None) | Payload::Empty => rendered(Str::new("")),
            Payload::Message(message) => Ok(message.clone()),
            Payload::Arguments(arguments) => match &arguments.os_paths {
                Some(paths) => paths.message(&arguments.message),
                None => Ok(arguments.message.clone()),
            },
            Payload::Json(e) => {
                let mut text = String::new();
                write!(
                    super::values::ReprWriter(&mut text),
                    "{}: line {} column {} (char {})",
                    e.msg,
                    e.lineno,
                    e.colno,
                    e.pos
                )?;
                rendered(text.into())
            }
            Payload::Process(process) => rendered(process.message()?),
            Payload::Timeout(timeout) => rendered(timeout.message()?),
            Payload::Key(key) => super::boxed_values::word_text(key.kind, key.word, true),
            Payload::Tree(errors) => errors.message(),
            Payload::Group(members) => {
                let count = members.len();
                let suffix = if count == 1 { "" } else { "s" };
                let mut text = String::new();
                write!(
                    super::values::ReprWriter(&mut text),
                    "unhandled errors in a TaskGroup ({count} sub-exception{suffix})"
                )?;
                rendered(text.into())
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
                    0 => StrOwner::into_raw(e.msg.clone()) as usize,
                    1 => StrOwner::into_raw(e.doc.clone()) as usize,
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

/// Construct SystemExit with its original code. Kind 4 denotes an omitted
/// argument; 0, 1, 2, 3 and 6 represent None, bool, int, str and tuple.
///
/// # Safety
/// The caller roots the scalar, string or tuple described by `kind`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_system_exit_new(kind: u32, word: usize) -> *const Object {
    let code = (kind != 4).then(|| {
        // SAFETY: the caller roots the argument through this acquisition.
        unsafe { WordKind::new(kind).acquire(word) }
    });
    crate::ObjectOwner::into_raw(new_exception(Kind::SystemExit, Payload::Exit(code)))
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
    crate::ObjectOwner::into_raw(unsafe { crate::ObjectOwner::acquire_word(value as usize) })
}

pub(super) unsafe fn exit_word(value: *const Object) -> (WordKind, usize) {
    // SAFETY: the caller roots the expected boxed or exception value.
    unsafe {
        (*value)
            .0
            .with(projected_word)
            .expect("checked boxed value")
    }
}

/// Borrow the concrete Python value from a box, scalar module member or exit-code view.
pub(super) fn projected_word(storage: &Storage) -> Option<(WordKind, usize)> {
    match storage {
        Storage::ObjectValue(v) => Some((v.kind, v.word)),
        Storage::ModuleMember(m) => m.scalar().map(|(k, w)| (WordKind::new(k), w)),
        Storage::Exception(e) => match &e.payload {
            Payload::Exit(Some(code)) => Some((code.kind, code.word)),
            Payload::ExitArguments(args) => Some((args.0.kind, args.0.word)),
            Payload::Exit(None) => Some((WordKind::new(0), 0)),
            _ => None,
        },
        _ => None,
    }
}

/// Identify the scalar type for an isinstance check on the original code.
///
/// # Safety
/// The caller roots a code view from `imre_system_exit_code`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_system_exit_code_kind(value: *const Object) -> u32 {
    // SAFETY: the caller roots the boxed value and any typed index throughout this borrow.
    if let Some(kind) = unsafe { super::errno::with_nonscalar_member(object(value), |m| m.kind()) }
    {
        return kind;
    }
    // Kind 5 is shared by native containers. A projected list or child
    // exception is not a dictionary just because it has an Object owner.
    // SAFETY: the view owns this Object throughout the inspection.
    unsafe {
        let (kind, word) = exit_word(value);
        if kind.raw() == 5 {
            return object(word as *const Object).0.with(|s| match s {
                Storage::Sequence(_) => 4,
                Storage::Dictionary(_) => 5,
                Storage::Set(_) => 12,
                Storage::File(file) => file.memory_class().map_or(15, |(_, kind)| kind),
                _ => 15,
            });
        }
        if kind.raw() == 6
            && super::records::descriptor(word as *const usize)
                .class_semantics()
                .is_some()
        {
            return 15; // a nominal record does not implement the tuple protocol
        }
        kind.raw()
    }
}

/// Render an exit code as a Python value, including None for a missing argument.
///
/// # Safety
/// `value` is a rooted code view returned by `imre_system_exit_code`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_system_exit_code_text(value: *const Object) -> *const Str {
    // SAFETY: the caller roots the boxed value and any typed index throughout this borrow.
    if let Some(text) = unsafe { super::errno::with_nonscalar_member(object(value), |m| m.text(0)) }
    {
        return text;
    }
    // SAFETY: the caller roots the immutable exit payload.
    unsafe {
        let (kind, word) = exit_word(value);
        super::boxed_values::checked_text(kind, word, false)
    }
}

/// Compare a retained field against another Python value (including a field view).
///
/// # Safety
/// `value` is a rooted code view. `word` is rooted with kind 0 (None), 1
/// (bool), 2 (int), 3 (str), 5 (another object view), 6 (tuple), 8 (bytes), or
/// 17 (a concrete dictionary or memory stream), or 18 (a typed list).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_system_exit_code_equal(
    value: *const Object,
    kind: u32,
    word: usize,
) -> u32 {
    // The compiler uses 5 for a second boxed view and 17 for a concrete
    // native object. Other representation tags identify scalar and tuple words.
    // SAFETY: both inputs stay rooted while we compare their borrowed fields.
    unsafe {
        // 18 is a concrete generic list. Its ordinary storage kind (5) is also
        // used for a boxed comparison operand, whose representation is different.
        if kind == 18 {
            if super::errno::with_nonscalar_member(object(value), |_| ()).is_some() {
                return 0;
            }
            return u32::from(super::boxed_values::equal(
                exit_word(value),
                (WordKind::new(5), word),
            ));
        }
        if super::errno::with_nonscalar_member(object(value), |_| ()).is_some() {
            let (other, payload) = if kind == 5 {
                if super::errno::with_nonscalar_member(object(word as *const Object), |_| ())
                    .is_some()
                {
                    (16, word)
                } else {
                    let (other, payload) = exit_word(word as *const Object);
                    if other.raw() == 5 {
                        return 0;
                    }
                    (other.raw(), payload)
                }
            } else {
                (if kind == 17 { 5 } else { kind }, word)
            };
            return super::errno::imre_errno_member_equal(value, other, payload);
        }
        if kind == 17 {
            return u32::from(super::boxed_values::equal(
                exit_word(value),
                (WordKind::new(5), word),
            ));
        }
        if kind == 5
            && super::errno::with_nonscalar_member(object(word as *const Object), |_| ()).is_some()
        {
            let (other, payload) = exit_word(value);
            if other.raw() == 5 {
                return 0;
            }
            return super::errno::imre_errno_member_equal(
                word as *const Object,
                other.raw(),
                payload,
            );
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
        ((k, w), (j, v))
    };
    // SAFETY: matching immutable scalar kinds describe both words.
    u32::from(unsafe { super::boxed_values::equal((left_kind, left), (right_kind, right)) })
}

/// # Safety
/// `value` is a rooted immutable exit code.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_system_exit_code_truth(value: *const Object) -> u32 {
    // SAFETY: the caller roots the boxed value and any typed index throughout this borrow.
    if let Some(bit) = unsafe { super::errno::with_nonscalar_member(object(value), |m| m.truth()) }
    {
        return bit;
    }
    // SAFETY: the code view roots the scalar described by the stored word kind.
    let (kind, word) = unsafe { exit_word(value) };
    u32::from(match kind.raw() {
        0 => false,
        1 => word != 0,
        2 => word != 1, // tagged zero
        // SAFETY: the immutable exit code owns this string.
        3 => !unsafe { &*(word as *const Str) }.is_empty(),
        4 | 5 | 10 => super::boxed_values::python_length(kind, word)
            .map_or_else(|error| error.returning(false), |length| length != Some(0)),
        7 => f64::from_bits(word as u64) != 0.0,
        // SAFETY: the view owns this exact byte buffer.
        8 => !unsafe { &*(word as *const super::Bytes) }.is_empty(),
        6 => {
            // SAFETY: the view roots the record and its immutable descriptor.
            let layout = unsafe { super::records::descriptor(word as *const usize) };
            layout.class_semantics().is_some() || layout.length != 0
        }
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
    take_pending().map_or(std::ptr::null(), crate::ObjectOwner::into_raw)
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
        crate::ObjectOwner::into_raw(value)
    }
}

/// Attach the completed Python field layout to the canonical exception object.
///
/// # Safety
/// The descriptor and method addresses are static. The compiler supplies a rooted
/// complete record and unary record-to-str methods. When flag 4 is set, the
/// argument is a rooted view of the positional tuple; otherwise it is a string or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_named_exception_record_new(
    descriptor: *const NamedDescriptor,
    record: *const usize,
    argument: *const (),
    text: Option<Formatter>,
    repr: Option<Formatter>,
    display_flags: u32,
) -> *const Object {
    // SAFETY: the fresh exception cannot yet alias user code and its payload
    // acquires the exact native record before the caller releases that owner.
    unsafe {
        let tuple = display_flags & 4 != 0;
        let value = imre_named_exception_new(
            descriptor,
            if tuple {
                std::ptr::null()
            } else {
                argument.cast()
            },
        );
        let positional = tuple.then(|| {
            let (kind, word) = exit_word(argument.cast());
            debug_assert_eq!(kind.raw(), 6);
            imports::Positional(kind.acquire(word))
        });
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
            if let Some(args) = positional {
                e.payload = Payload::Positional(args);
            }
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

pub(super) type Rendered = std::result::Result<StrOwner, std::fmt::Error>;
pub(super) fn rendered(text: Str) -> Rendered {
    StrOwner::try_new(text).map_err(|_| std::fmt::Error)
}

pub(super) fn finish_display(result: Rendered) -> *const Str {
    if imre_error_pending() != 0 {
        return std::ptr::null();
    }
    finish(
        result
            .map(StrOwner::into_raw)
            .map_err(|_| Error::allocation()),
        std::ptr::null(),
    )
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
            return unsafe {
                super::boxed_values::checked_text(layout.record.kind, layout.record.word, true)
            };
        }
    }
    use std::fmt::Write as _;
    // SAFETY: release the storage borrow before a nested display method can mutate the exception.
    let snapshot = match unsafe { object(value).0.with(|s| s.exception().display_snapshot()) } {
        Ok(exception) => Storage::Exception(exception),
        Err(error) => {
            error.raise();
            return std::ptr::null();
        }
    };
    let s = &snapshot;
    // SAFETY: the snapshot retains the arguments throughout reentrant formatting.
    let result = unsafe {
        (|| {
            if !repr {
                return s.exception_message();
            }
            let exception = s.exception();
            let mut result = String::new();
            let mut output = super::values::ReprWriter(&mut result);
            let name = exception.name();
            output.write_str(name.rsplit('.').next().unwrap_or(name))?;
            output.write_char('(')?;
            match &exception.payload {
                Payload::Import(err) => err.args.write_repr_args(&mut output)?,
                Payload::Positional(args) | Payload::ExitArguments(args) => {
                    args.write_repr_args(&mut output)?
                }
                Payload::Tree(errors) => errors.write_message(&mut output)?,
                Payload::Empty | Payload::Exit(None) => {}
                Payload::Arguments(arguments) => {
                    for (index, arg) in arguments.args.iter().enumerate() {
                        if index != 0 {
                            output.write_str(", ")?;
                        }
                        super::values::try_write_repr(&mut output, arg.word, arg.kind)?;
                    }
                }
                Payload::Key(key) | Payload::Exit(Some(key)) => {
                    super::values::try_write_repr(&mut output, key.word, key.kind)?;
                }
                Payload::Process(_) | Payload::Timeout(_) | Payload::Group(_) => {
                    arguments::write_projected_repr(value, &mut output)?;
                }
                _ => write_quoted_repr(s.exception_message()?.as_str(), &mut output)?,
            }
            output.write_char(')')?;
            rendered(result.into())
        })()
    };
    finish_display(result)
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
        // SAFETY: the non-null argument is a rooted StrOwner.
        Payload::Message(unsafe { crate::StrOwner::acquire_word(message as usize) })
    };
    crate::ObjectOwner::into_raw(new_exception(kind, payload))
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

/// The compiler emits only known builtin IDs. Use the same hierarchy as a catch.
#[unsafe(no_mangle)]
pub extern "C" fn imre_builtin_exception_subclass(child: u32, parent: u32) -> bool {
    Kind::from_raw(child).matches(Kind::from_raw(parent))
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
                && d.mro().contains(&expected_kind)
            {
                return 1;
            }
            u32::from(expected_kind < 256 && e.kind.matches(Kind::from_raw(expected_kind)))
        })
    }
}

/// Match an exception contained in an erased field, including a task group's original member.
///
/// # Safety
/// The caller roots a boxed Python value; the expected category is compiler-validated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_exception_field_matches(
    value: *const Object,
    expected_kind: u32,
) -> u32 {
    // SAFETY: both the view and any projected exception remain rooted across these reads.
    unsafe {
        let Some((kind, word)) = object(value).0.with(projected_word) else {
            return 0;
        };
        let child = word as *const Object;
        if kind.raw() == 5 && object(child).0.with(|s| matches!(s, Storage::Exception(_))) {
            imre_exception_matches(child, expected_kind)
        } else {
            0
        }
    }
}

/// Match the categories retained by a previously constructed assertion context.
///
/// # Safety
/// Both arguments must be live handles of their declared types. A category lookup
/// only reads the exception and cannot run Python or mutate the list during the borrow.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_exception_matches_any(
    value: *const Object,
    kinds: *const crate::List,
) -> u32 {
    // SAFETY: the pointers are independently rooted throughout these readonly operations.
    u32::from(unsafe {
        (*kinds).0.with(|entries| {
            entries.iter().any(|entry| {
                entry
                    .as_small()
                    .and_then(|n| u32::try_from(n).ok())
                    .is_some_and(|kind| {
                        (kind >= 256 || Kind::try_from_raw(kind).is_some())
                            && imre_exception_matches(value, kind) != 0
                    })
            })
        })
    })
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

/// Record a Python raise site without changing the exception's message or identity.
///
/// # Safety
/// Both inputs are live borrowed Imre values. The result owns an additional
/// reference to the same exception.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_exception_with_origin(
    value: *const Object,
    filename: *const Str,
    line: u64,
) -> *const Object {
    // SAFETY: both input owners outlive this call. A stored filename owns its reference.
    unsafe {
        object(value).0.with_mut(|storage| {
            let Storage::Exception(exception) = storage else {
                fail("SystemError", "expected an exception object");
            };
            let Ok(details) = exception.try_details_mut() else {
                return;
            };
            let origins = &mut details.origins;
            if origins.try_reserve(1).is_ok() {
                crate::imre_str_retain(filename);
                origins.push((StrOwner::from_raw(filename), line));
            }
        });
        imre_exception_retain(value);
    }
    value
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
            exception.set_explicit_cause(cause);
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
            .map_or(std::ptr::null(), crate::ObjectOwner::into_raw)
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
            .map_or(std::ptr::null(), crate::ObjectOwner::into_raw)
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
    ACTIVE.with(|active| {
        active
            .replace(new)
            .map_or(std::ptr::null(), crate::ObjectOwner::into_raw)
    })
}

/// # Safety
/// `value` must be a rooted, non-null exception. Pending state acquires a reference.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_exception_raise(value: *const Object) {
    if value.is_null() {
        fail("SystemError", "cannot raise a null exception");
    }
    // SAFETY: the caller supplies a rooted exception.
    publish_raised(unsafe { acquire_exception(value) }.unwrap());
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

/// An integer passed to SystemExit first has to fit the platform's C long.
#[derive(Clone, Debug)]
pub(crate) enum ExitStatus {
    Code(i32),
    Overflow,
}

impl ExitStatus {
    pub(crate) fn from_i64(value: Option<i64>) -> Self {
        value.map_or(Self::Overflow, |code| Self::Code(code as i32))
    }

    pub(crate) fn report(self) -> i32 {
        match self {
            Self::Code(code) => code,
            Self::Overflow => {
                eprintln!("OverflowError: Python int too large to convert to C long");
                -1
            }
        }
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
    enum ExitText {
        Original(StrOwner),
        Rendered(String),
    }
    // SAFETY: snapshot the owned exit code before formatting can call Python.
    let code = unsafe {
        pending
            .0
            .with(|storage| match &storage.exception().payload {
                Payload::Exit(code) => Some(code.clone()),
                Payload::ExitArguments(args) => Some(Some(args.0.clone())),
                _ => None,
            })
    };
    let exit = code.map(|code| match code.as_ref() {
        None => (0, None),
        Some(code) if code.kind == WordKind::new(0) => (0, None),
        Some(code) if code.kind == WordKind::new(1) => (code.word as i32, None),
        Some(code) if code.kind == WordKind::new(2) => {
            // SAFETY: the snapshot owns the exact integer. CPython uses -1 on overflow.
            let integer = unsafe { crate::Int::borrow(code.word) };
            (integer.big().to_i64().unwrap_or(-1) as i32, None)
        }
        Some(code) => {
            let message = if code.kind == WordKind::new(3) {
                // SAFETY: the code owns an exact, live string.
                Some(ExitText::Original(unsafe {
                    StrOwner::acquire_word(code.word)
                }))
            } else if code.kind == WordKind::new(5)
                && unsafe {
                    // SAFETY: the object snapshot is live; this borrow ends before __str__.
                    object(code.word as *const Object)
                        .0
                        .with(|v| matches!(v, Storage::Exception(_)))
                }
            {
                // SAFETY: the snapshot roots the checked exception.
                let text = unsafe { imre_exception_message(code.word as *const Object) };
                // SAFETY: a non-null formatter result transfers one string owner.
                (!text.is_null()).then(|| ExitText::Original(unsafe { StrOwner::from_raw(text) }))
            } else {
                super::boxed_values::word_repr(code.kind, code.word)
                    .ok()
                    .map(ExitText::Rendered)
            };
            if message.is_none() {
                // CPython discards an error raised while writing the exit diagnostic.
                drop(take_pending());
            }
            (1, Some(message))
        }
    });
    let stderr = crate::sys::flush_stderr();
    let stdout = crate::flush_stdout();
    if let Some((status, message)) = exit {
        if let Some(message) = message {
            let text = match &message {
                Some(ExitText::Original(text)) => text.as_str(),
                Some(ExitText::Rendered(text)) => text.as_str(),
                None => "",
            };
            let _ = crate::sys::write_stderr(text, true);
        }
        return status_after_flush(status, stdout, stderr.and(crate::sys::flush_stderr()));
    }
    write_exception_chain(pending);
    status_after_flush(1, stdout, stderr.and(crate::sys::flush_stderr()))
}

/// Print the handled exception without changing the active handler or the process exit status.
#[unsafe(no_mangle)]
pub extern "C" fn imre_log_print_active_exception() {
    if let Some(exception) = active_exception() {
        write_exception_chain(exception);
    } else {
        let _ = crate::sys::write_stderr("NoneType: None", true);
    }
}

/// Render the exception and its explicit causes without retaining any native frames.
fn write_exception_chain(exception: crate::ObjectOwner) {
    let mut chain = Vec::new();
    let mut current = Some(exception.clone());
    while let Some(exception) = current {
        // SAFETY: the current owner roots the exception; only owned data escapes.
        let (name, origins) = unsafe {
            exception.0.with(|storage| {
                let e = storage.exception();
                (
                    e.name().to_owned(),
                    e.details.as_deref().map(|details| details.origins.clone()),
                )
            })
        };
        let parent = display_parent(&exception);
        let explicit = parent.as_ref().is_some_and(|(_, explicit)| *explicit);
        // SAFETY: the local owner outlives custom formatting and any resulting error.
        let message = unsafe { display(crate::ObjectOwner::as_ptr(&exception), false) };
        let message = if message.is_null() {
            drop(take_pending());
            StrOwner::new(Str::new("<exception str() failed>"))
        } else {
            // SAFETY: display transfers one owned string.
            unsafe { StrOwner::from_raw(message) }
        };
        // Traceback suggestions decorate only uncaught output. The owned exception's
        // message, positional arguments, str() and repr() stay independent.
        // SAFETY: the owned exception stays live; this read does not invoke callbacks.
        let suggestion = unsafe {
            exception
                .0
                .with(|storage| match &storage.exception().payload {
                    Payload::Import(fields) => fields.suggestion(),
                    _ => None,
                })
        };
        chain.push((name, message, suggestion, origins, explicit));
        current = parent.map(|(parent, _)| parent);
    }
    drop(exception);
    let mut diagnostic = crate::sys::stderr_writer();
    for (index, (name, message, suggestion, origins, explicit)) in
        chain.into_iter().rev().enumerate()
    {
        if index != 0 {
            let separator = if explicit {
                "The above exception was the direct cause of the following exception:"
            } else {
                "During handling of the above exception, another exception occurred:"
            };
            let _ = writeln!(diagnostic, "\n{separator}\n");
        }
        if let Some(origins) = origins.filter(|frames| !frames.is_empty()) {
            let _ = writeln!(diagnostic, "Traceback (most recent call last):");
            for (filename, line) in origins.iter().rev() {
                let _ = writeln!(diagnostic, "  File \"{filename}\", line {line}");
            }
        }
        if let Some(hint) = suggestion {
            let _ = writeln!(diagnostic, "{name}: {message}. Did you mean: '{hint}'?");
        } else if message.is_empty() {
            let _ = writeln!(diagnostic, "{name}");
        } else {
            let _ = writeln!(diagnostic, "{name}: {message}");
        }
    }
}

/// CPython reports a stdout flush error, silently records a stderr flush error,
/// and uses status 120 if either stream could not be finalized.
pub(crate) fn status_after_flush(
    status: i32,
    stdout: io::Result<()>,
    stderr: io::Result<()>,
) -> i32 {
    if let Err(error) = stdout {
        let broken_pipe = error.kind() == io::ErrorKind::BrokenPipe;
        let error = super::files::errno_error(error);
        let name = if broken_pipe {
            "BrokenPipeError"
        } else {
            error.kind.name()
        };
        let exception = error.into_object();
        // SAFETY: the owned OS exception roots its arguments throughout formatting.
        let message = unsafe { exception.0.with(Storage::exception_message) };
        let message = message
            .as_deref()
            .map_or("<exception str() failed>", Str::as_str);
        let mut diagnostic = io::stderr().lock();
        let _ = writeln!(diagnostic, "Exception ignored while flushing sys.stdout:");
        if message.is_empty() {
            let _ = writeln!(diagnostic, "{name}");
        } else {
            let _ = writeln!(diagnostic, "{name}: {message}");
        }
        120
    } else if stderr.is_err() {
        120
    } else {
        status
    }
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
    use crate::shared::Shared;
    use crate::{
        Int, imre_int_binary, imre_int_parse, imre_list_append, imre_list_copy_index,
        imre_list_get_index, imre_list_new, imre_list_pop, imre_list_repeat,
    };
    use num_bigint::BigInt;

    #[test]
    fn implicit_context_releases_indirectly_owned_group_members() {
        let group = |child| {
            new_exception(
                Kind::ExceptionGroup,
                Payload::Group(vec![GroupMember::Strong(child)].into_boxed_slice()),
            )
        };
        let released = [false, true].map(|through_cause| {
            let leaf = new_exception(Kind::ValueError, Payload::Empty);
            let weak = crate::ObjectOwner::downgrade(&leaf);
            let child = if through_cause {
                let child = new_exception(Kind::RuntimeError, Payload::Empty);
                // SAFETY: the fresh child cannot be reached from the leaf.
                unsafe {
                    child.0.with_mut(|storage| {
                        let Storage::Exception(e) = storage else {
                            unreachable!()
                        };
                        e.details_mut().cause = Some(leaf.clone());
                        e.suppress_context = true;
                    })
                };
                child
            } else {
                group(leaf.clone())
            };
            assert!(attach_display_context(&leaf, group(child)).is_ok());
            assert!(display_parent(&leaf).is_some_and(|(_, explicit)| !explicit));
            drop(leaf);
            weak.upgrade().is_none()
        });
        assert_eq!(released, [true, true]);
    }

    #[test]
    fn cached_memory_errors_keep_separate_directory_owners() {
        use crate::objects::tempdirs::{imre_directory_context_new, imre_tempdir_new};

        fn retained(raise: fn()) -> (crate::ObjectOwner, StrOwner) {
            let prefix = Str::from("imre-error-");
            let suffix = Str::from("");
            // SAFETY: both names are rooted and a null directory selects the system default.
            let name = unsafe { imre_tempdir_new(std::ptr::null(), &prefix, &suffix) };
            assert!(!name.is_null());
            // SAFETY: creation transfers one string owner. The manager acquires its own reference.
            let (name, manager) = unsafe {
                let manager = imre_directory_context_new(name as usize, 1);
                (
                    StrOwner::from_raw(name),
                    crate::ObjectOwner::from_raw(manager),
                )
            };
            // SAFETY: the compiler roots the manager while preparing its one traceback slot.
            let frame = unsafe {
                imre_prepare_directory_frame(
                    std::ptr::null(),
                    crate::ObjectOwner::as_ptr(&manager),
                    1,
                )
            };
            raise();
            let alias = PENDING.with_borrow(|pending| pending.as_ref().unwrap().clone());
            // SAFETY: the compiler transfers one private frame after visiting its single local.
            unsafe {
                imre_error_frame_keep_directory(frame, crate::ObjectOwner::as_ptr(&manager));
                imre_error_attach_directory_frame(frame);
            }
            let error = take_pending().unwrap();
            assert!(crate::ObjectOwner::ptr_eq(&alias, &error));
            (error, name)
        }

        for raise in [record_allocation_failed as fn(), sequence_allocation_failed] {
            let (first, left) = retained(raise);
            let (second, right) = retained(raise);
            assert!(!crate::ObjectOwner::ptr_eq(&first, &second));
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
        let child = new_exception(Kind::ValueError, StrOwner::new(Str::from("child")));
        let group = task_group_errors(vec![child.clone(), child.clone()]);
        let weak_group = crate::ObjectOwner::downgrade(&group);
        let weak_child = crate::ObjectOwner::downgrade(&child);
        // SAFETY: local owners root this exact group and one of its members.
        unsafe {
            let raised = crate::ObjectOwner::from_raw(imre_async_member_cause(
                crate::ObjectOwner::as_ptr(&child),
                crate::ObjectOwner::as_ptr(&group),
            ));
            assert!(crate::ObjectOwner::ptr_eq(&child, &raised));
            let first = crate::ObjectOwner::from_raw(imre_async_exception_at(
                crate::ObjectOwner::as_ptr(&group),
                1,
            ));
            assert!(crate::ObjectOwner::ptr_eq(&first, &raised));
            drop((first, raised, child));
            assert!(weak_child.upgrade().is_none());
            let first = crate::ObjectOwner::from_raw(imre_async_exception_at(
                crate::ObjectOwner::as_ptr(&group),
                1,
            ));
            let second = crate::ObjectOwner::from_raw(imre_async_exception_at(
                crate::ObjectOwner::as_ptr(&group),
                3,
            ));
            assert!(crate::ObjectOwner::ptr_eq(&first, &second));
            let cause = crate::ObjectOwner::from_raw(imre_exception_cause(
                crate::ObjectOwner::as_ptr(&second),
            ));
            assert!(crate::ObjectOwner::ptr_eq(&cause, &group));
            drop((first, second, cause, group));
            assert!(weak_group.upgrade().is_none());
        }
    }

    fn take_error(kind: Kind) -> crate::ObjectOwner {
        assert_eq!(imre_error_pending(), 1);
        let value = imre_error_take();
        assert!(!value.is_null());
        assert_eq!(imre_error_pending(), 0);
        // SAFETY: take transfers a non-null owned exception reference.
        let value = unsafe { crate::ObjectOwner::from_raw(value) };
        // SAFETY: the owned exception is rooted and the expected kind is valid.
        let matches =
            unsafe { imre_exception_matches(crate::ObjectOwner::as_ptr(&value), kind as u32) };
        assert_eq!(matches, 1);
        value
    }

    fn message(value: &crate::ObjectOwner) -> StrOwner {
        // SAFETY: the rooted exception accessor transfers one string reference.
        unsafe { StrOwner::from_raw(imre_exception_message(crate::ObjectOwner::as_ptr(value))) }
    }

    #[test]
    fn key_error_owns_its_key_until_the_exception_is_released() {
        let key = StrOwner::new(Str::new("a quoted 'key'\n"));
        let expected = string_repr(&key);
        let weak = StrOwner::downgrade(&key);
        // SAFETY: the key is rooted and the constructor returns one owned exception.
        let error = unsafe {
            crate::ObjectOwner::from_raw(imre_exception_new(
                Kind::KeyError as u32,
                StrOwner::as_ptr(&key),
            ))
        };
        drop(key);
        assert_eq!(weak.strong_count(), 1);
        assert_eq!(*message(&error), expected);
        drop(error);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn explicit_causes_preserve_identity_and_independent_ownership() {
        let original = new_exception(Kind::KeyError, StrOwner::new(Str::new("original")));
        let wrapped = new_exception(Kind::ValueError, StrOwner::new(Str::new("wrapped")));
        let weak = crate::ObjectOwner::downgrade(&original);
        // SAFETY: wrapped is fresh; every borrowed input remains rooted and each
        // transferred reference is adopted immediately.
        unsafe {
            assert!(imre_exception_cause(crate::ObjectOwner::as_ptr(&wrapped)).is_null());
            assert_eq!(
                imre_exception_suppress_context(crate::ObjectOwner::as_ptr(&wrapped)),
                0
            );
            let result = crate::ObjectOwner::from_raw(imre_exception_with_cause(
                crate::ObjectOwner::as_ptr(&wrapped),
                crate::ObjectOwner::as_ptr(&original),
            ));
            assert!(crate::ObjectOwner::ptr_eq(&result, &wrapped));
            assert_eq!(
                imre_exception_suppress_context(crate::ObjectOwner::as_ptr(&wrapped)),
                1
            );
            let cause = crate::ObjectOwner::from_raw(imre_exception_cause(
                crate::ObjectOwner::as_ptr(&wrapped),
            ));
            assert!(crate::ObjectOwner::ptr_eq(&cause, &original));
            drop((original, wrapped, result));
            assert_eq!(message(&cause).as_str(), "original");
            drop(cause);
            assert!(weak.upgrade().is_none());

            let suppressed = new_exception(Kind::ValueError, StrOwner::new(Str::new("hidden")));
            drop(crate::ObjectOwner::from_raw(imre_exception_with_cause(
                crate::ObjectOwner::as_ptr(&suppressed),
                std::ptr::null(),
            )));
            assert!(imre_exception_cause(crate::ObjectOwner::as_ptr(&suppressed)).is_null());
            assert_eq!(
                imre_exception_suppress_context(crate::ObjectOwner::as_ptr(&suppressed)),
                1
            );
        }
    }

    #[test]
    fn deep_cause_chains_release_without_recursive_drop() {
        let message = StrOwner::new(Str::new("chain"));
        let mut tail = new_exception(Kind::ValueError, message.clone());
        let weak_first = crate::ObjectOwner::downgrade(&tail);
        for _ in 0..100_000 {
            let next = new_exception(Kind::ValueError, message.clone());
            // SAFETY: next is fresh and tail remains rooted until attachment.
            unsafe {
                drop(crate::ObjectOwner::from_raw(imre_exception_with_cause(
                    crate::ObjectOwner::as_ptr(&next),
                    crate::ObjectOwner::as_ptr(&tail),
                )));
            }
            tail = next;
        }
        drop(tail);
        assert!(weak_first.upgrade().is_none());
        assert_eq!(StrOwner::strong_count(&message), 1);
    }

    #[test]
    fn pending_and_active_errors_transfer_independent_owned_references() {
        let text = StrOwner::new(Str::new("outer"));
        let weak_text = StrOwner::downgrade(&text);
        // SAFETY: the argument is a rooted string; construction transfers an owner.
        let outer = unsafe {
            crate::ObjectOwner::from_raw(imre_exception_new(
                Kind::ValueError as u32,
                StrOwner::as_ptr(&text),
            ))
        };
        let weak_outer = crate::ObjectOwner::downgrade(&outer);
        drop(text);
        // SAFETY: all exception inputs are rooted and every transferred owner is
        // adopted or released below, including same-value pending replacement.
        unsafe {
            imre_exception_raise(crate::ObjectOwner::as_ptr(&outer));
            imre_error_set(crate::ObjectOwner::as_ptr(&outer));
            let pending = take_error(Kind::ValueError);
            assert!(crate::ObjectOwner::ptr_eq(&outer, &pending));
            assert!(imre_exception_swap_active(crate::ObjectOwner::as_ptr(&outer)).is_null());
            drop(pending);
            let inner = crate::ObjectOwner::from_raw(imre_exception_new(
                Kind::IndexError as u32,
                std::ptr::null(),
            ));
            let previous = crate::ObjectOwner::from_raw(imre_exception_swap_active(
                crate::ObjectOwner::as_ptr(&inner),
            ));
            assert!(crate::ObjectOwner::ptr_eq(&outer, &previous));
            drop(outer);
            let current = crate::ObjectOwner::from_raw(imre_exception_current());
            assert!(crate::ObjectOwner::ptr_eq(&inner, &current));
            imre_exception_reraise();
            let raised = take_error(Kind::IndexError);
            assert!(crate::ObjectOwner::ptr_eq(&inner, &raised));
            let displaced = crate::ObjectOwner::from_raw(imre_exception_swap_active(
                crate::ObjectOwner::as_ptr(&previous),
            ));
            assert!(crate::ObjectOwner::ptr_eq(&inner, &displaced));
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
        let first = new_exception(Kind::ValueError, StrOwner::new(Str::new("first")));
        let second = new_exception(Kind::IndexError, StrOwner::new(Str::new("second")));
        let weak_first = crate::ObjectOwner::downgrade(&first);
        let weak_second = crate::ObjectOwner::downgrade(&second);
        // SAFETY: inputs stay rooted through acquisition, then pending state
        // becomes their sole owner. Null explicitly clears that owner.
        unsafe {
            imre_error_set(crate::ObjectOwner::as_ptr(&first));
            drop(first);
            assert_eq!(imre_error_pending(), 1);
            imre_error_set(crate::ObjectOwner::as_ptr(&second));
            drop(second);
            assert!(weak_first.upgrade().is_none());
            assert_eq!(imre_error_pending(), 1);
            imre_error_set(std::ptr::null());
        }
        assert!(weak_second.upgrade().is_none());
        assert_eq!(imre_error_pending(), 0);
        assert!(imre_error_take().is_null());

        let reported = new_exception(
            Kind::RuntimeError,
            StrOwner::new(Str::new("reported test error")),
        );
        let weak_reported = crate::ObjectOwner::downgrade(&reported);
        // SAFETY: pending and active independently acquire this rooted exception.
        unsafe {
            imre_error_set(crate::ObjectOwner::as_ptr(&reported));
            assert!(imre_exception_swap_active(crate::ObjectOwner::as_ptr(&reported)).is_null());
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
                let value = crate::ObjectOwner::from_raw(imre_exception_new(
                    actual as u32,
                    std::ptr::null(),
                ));
                assert_eq!(
                    imre_exception_matches(crate::ObjectOwner::as_ptr(&value), expected as u32),
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
            let argument = argument.map(|value| StrOwner::new(Str::new(value)));
            let pointer = argument.as_ref().map_or(std::ptr::null(), StrOwner::as_ptr);
            // SAFETY: the nullable argument remains rooted throughout construction.
            let error = unsafe {
                crate::ObjectOwner::from_raw(imre_exception_new(Kind::KeyError as u32, pointer))
            };
            drop(argument);
            assert_eq!(message(&error).as_str(), expected);
        }
    }

    #[test]
    fn caught_arithmetic_and_list_failures_preserve_owners_and_allow_recovery() {
        let big = Shared::new(BigInt::from(1_u8) << 120_u32);
        let weak = Shared::downgrade(&big);
        let integer = Int(Shared::into_raw(big) as usize);
        // SAFETY: the constructor transfers one owned list reference.
        let list = unsafe { crate::ListOwner::from_raw(imre_list_new()) };
        let zero = Int::small(0).unwrap();
        // SAFETY: all integer/list inputs stay rooted, and each successful owned
        // result is adopted exactly once. Failure sentinels are never acquired.
        unsafe {
            assert_eq!(imre_int_binary(integer.0, zero.0, 3), 1);
            drop(take_error(Kind::ZeroDivisionError));
            assert_eq!(imre_int_parse(&Str::new("12__3")), 1);
            drop(take_error(Kind::ValueError));
            imre_list_append(crate::ListOwner::as_ptr(&list), integer.0);
            drop(integer);
            assert_eq!(weak.strong_count(), 1);
            imre_list_copy_index(crate::ListOwner::as_ptr(&list), 1, 0);
            drop(take_error(Kind::IndexError));
            assert_eq!(weak.strong_count(), 1);
            assert_eq!(imre_list_get_index(crate::ListOwner::as_ptr(&list), 7), 0);
            drop(take_error(Kind::IndexError));
            let huge = Int::new(BigInt::from(1_u8) << 100_u32);
            assert!(imre_list_repeat(crate::ListOwner::as_ptr(&list), huge.0).is_null());
            drop(take_error(Kind::OverflowError));
            let capacity = Int::new(BigInt::from(isize::MAX));
            assert!(imre_list_repeat(crate::ListOwner::as_ptr(&list), capacity.0).is_null());
            drop(take_error(Kind::MemoryError));
            let popped = Int(imre_list_pop(crate::ListOwner::as_ptr(&list)));
            assert_eq!(popped.big().as_ref(), &(BigInt::from(1_u8) << 120_u32));
            drop(popped);
            assert_eq!(imre_list_pop(crate::ListOwner::as_ptr(&list)), 0);
            drop(take_error(Kind::IndexError));
        }
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn failed_sequence_updates_release_staged_values_after_access() {
        let text = StrOwner::new(Str::new("staged"));
        let weak = StrOwner::downgrade(&text);
        // SAFETY: constructors transfer owners; all string/exception slots use
        // their declared representation and every getter's owner is adopted.
        unsafe {
            let sequence = crate::ObjectOwner::from_raw(imre_seq_new(3));
            imre_seq_set_index(
                crate::ObjectOwner::as_ptr(&sequence),
                0,
                StrOwner::as_ptr(&text) as usize,
                3,
            );
            drop(take_error(Kind::IndexError));
            assert_eq!(StrOwner::strong_count(&text), 1);
            assert_eq!(imre_seq_pop(crate::ObjectOwner::as_ptr(&sequence)), 0);
            drop(take_error(Kind::IndexError));
            drop(sequence);
            let sequence = crate::ObjectOwner::from_raw(imre_seq_new(5));
            let exception = crate::ObjectOwner::from_raw(imre_exception_new(
                Kind::Exception as u32,
                StrOwner::as_ptr(&text),
            ));
            let weak_exception = crate::ObjectOwner::downgrade(&exception);
            imre_seq_append(
                crate::ObjectOwner::as_ptr(&sequence),
                crate::ObjectOwner::as_ptr(&exception) as usize,
                5,
            );
            drop(exception);
            let exception = crate::ObjectOwner::from_raw(imre_seq_get_index(
                crate::ObjectOwner::as_ptr(&sequence),
                0,
            ) as *const Object);
            imre_exception_raise(crate::ObjectOwner::as_ptr(&exception));
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
        let string_key = StrOwner::new(Str::new("a'b\n"));
        let integer_key = Int::new(BigInt::from(1_u8) << 100_u32);
        // SAFETY: dictionary keys/values match their declared representations;
        // iterator output words are independent writable stack slots.
        unsafe {
            let strings = crate::ObjectOwner::from_raw(imre_dict_new(3, 2));
            assert_eq!(
                imre_dict_get(
                    crate::ObjectOwner::as_ptr(&strings),
                    StrOwner::as_ptr(&string_key) as usize
                ),
                0
            );
            assert_eq!(message(&take_error(Kind::KeyError)).as_str(), "\"a'b\\n\"");
            imre_dict_delete(
                crate::ObjectOwner::as_ptr(&strings),
                StrOwner::as_ptr(&string_key) as usize,
            );
            assert_eq!(message(&take_error(Kind::KeyError)).as_str(), "\"a'b\\n\"");
            let integers = crate::ObjectOwner::from_raw(imre_dict_new(2, 2));
            assert_eq!(
                imre_dict_get(crate::ObjectOwner::as_ptr(&integers), integer_key.0),
                0
            );
            assert_eq!(
                message(&take_error(Kind::KeyError)).as_str(),
                "1267650600228229401496703205376"
            );
            imre_dict_set(crate::ObjectOwner::as_ptr(&integers), integer_key.0, 1);
            let iterator = crate::ObjectOwner::from_raw(imre_dict_iter(
                crate::ObjectOwner::as_ptr(&integers),
                1,
            ));
            imre_dict_delete(crate::ObjectOwner::as_ptr(&integers), integer_key.0);
            let (mut key, mut value) = (17, 19);
            assert_eq!(
                imre_mapping_iter_next(crate::ObjectOwner::as_ptr(&iterator), &mut key, &mut value),
                0
            );
            drop(take_error(Kind::RuntimeError));
            assert_eq!((key, value), (17, 19));
            imre_dict_set(crate::ObjectOwner::as_ptr(&integers), integer_key.0, 1);
            assert_eq!(
                imre_mapping_iter_next(crate::ObjectOwner::as_ptr(&iterator), &mut key, &mut value),
                0
            );
            drop(take_error(Kind::RuntimeError));
            assert_eq!((key, value), (17, 19));
        }
    }

    #[test]
    fn windows_path_repr_preserves_other_escapes() {
        for raw in [
            r"\\server\share\folder",
            "C:\\one'\t\\two\"\n",
            "C:\\'\0\\\r\\𐐀\\\u{1faea}",
            r"C:\x00\next",
        ] {
            let mut rendered = String::from("prefix ");
            write_windows_path_repr(raw, &mut rendered).unwrap();
            assert_eq!(
                &rendered[7..],
                string_repr(&raw.replace('\\', "/")).as_str()
            );
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

    #[test]
    fn unhandled_exit_discards_failed_exception_text() {
        unsafe extern "C" fn failed_text(_: *const usize) -> *const Str {
            Error::new(Kind::TypeError, "display failed").raise();
            std::ptr::null()
        }

        assert_eq!(imre_error_pending(), 0);
        let code = new_exception(Kind::ValueError, Payload::Empty);
        // SAFETY: `code` is fresh and unescaped, and the failing formatter never
        // reads the dummy record word.
        unsafe {
            code.0.with_mut(|storage| {
                let Storage::Exception(exception) = storage else {
                    unreachable!()
                };
                exception.details_mut().layout = Some(Rc::new(NamedPayload {
                    record: OwnedWord::from_owned(0, WordKind::new(0)),
                    text: Some(failed_text),
                    repr: None,
                    dataclass_repr: false,
                    takes_record_owner: false,
                }));
            });
        }
        // SAFETY: the local object owner roots the exception during acquisition.
        let code_word =
            unsafe { WordKind::new(5).acquire(crate::ObjectOwner::as_ptr(&code) as usize) };
        replace_pending(Some(new_exception(
            Kind::SystemExit,
            Payload::Exit(Some(code_word)),
        )));

        assert_eq!(imre_error_unhandled(), 1);
        assert_eq!(imre_error_pending(), 0);
    }
}
