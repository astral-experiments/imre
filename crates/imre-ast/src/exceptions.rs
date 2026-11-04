//! Exception IDs shared by checked Python lowering and the native runtime.

macro_rules! exception_kinds {
    (
        boxed_builtins { $($builtin:ident = $builtin_id:literal,)* }
        other { $($other:ident = $other_id:literal => $display:literal,)* }
    ) => {
        /// Stable identifiers passed across the generated-code ABI.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[repr(u32)]
        pub enum Kind {
            $($builtin = $builtin_id,)*
            $($other = $other_id,)*
        }

        impl Kind {
            /// Decodes a native exception ID, leaving reserved IDs unassigned.
            pub fn try_from_raw(id: u32) -> Option<Self> {
                match id {
                    $($builtin_id => Some(Self::$builtin),)*
                    $($other_id => Some(Self::$other),)*
                    _ => None,
                }
            }

            pub fn name(self) -> &'static str {
                match self {
                    $(Self::$builtin => stringify!($builtin),)*
                    $(Self::$other => $display,)*
                }
            }
        }

        /// Names installed in the boxed builtins dictionary, in insertion order.
        pub const BOXED_BUILTINS: &[&str] = &[$(stringify!($builtin)),*];

        /// Resolves an exception exposed by the boxed builtins module.
        pub fn boxed_builtin_kind(name: &str) -> Option<Kind> {
            match name {
                $(stringify!($builtin) => Some(Kind::$builtin),)*
                _ => None,
            }
        }
    };
}

exception_kinds! {
    boxed_builtins {
        BaseException = 0,
        BaseExceptionGroup = 95,
        ExceptionGroup = 34,
        Exception = 1,
        RuntimeError = 10,
        RecursionError = 43,
        PythonFinalizationError = 73,
        NotImplementedError = 37,
        TypeError = 13,
        ValueError = 8,
        EOFError = 11,
        SyntaxError = 55,
        IndentationError = 56,
        TabError = 92,
        UnicodeError = 17,
        UnicodeDecodeError = 18,
        UnicodeEncodeError = 38,
        ArithmeticError = 2,
        FloatingPointError = 93,
        OverflowError = 4,
        ZeroDivisionError = 3,
        LookupError = 5,
        IndexError = 6,
        KeyError = 7,
        NameError = 39,
        UnboundLocalError = 40,
        AttributeError = 14,
        ImportError = 41,
        ModuleNotFoundError = 42,
        OSError = 12,
        InterruptedError = 50,
        ChildProcessError = 57,
        ProcessLookupError = 58,
        ConnectionError = 45,
        ConnectionRefusedError = 48,
        ConnectionResetError = 49,
        ConnectionAbortedError = 47,
        BrokenPipeError = 46,
        BlockingIOError = 44,
        TimeoutError = 51,
        FileNotFoundError = 20,
        FileExistsError = 24,
        IsADirectoryError = 22,
        NotADirectoryError = 23,
        PermissionError = 21,
        AssertionError = 9,
        MemoryError = 15,
        SystemError = 16,
        BufferError = 54,
        ReferenceError = 53,
        SystemExit = 32,
        GeneratorExit = 59,
        KeyboardInterrupt = 61,
        StopIteration = 35,
        StopAsyncIteration = 60,
        Warning = 80,
        RuntimeWarning = 81,
        DeprecationWarning = 82,
        EncodingWarning = 83,
        UserWarning = 84,
        PendingDeprecationWarning = 85,
        SyntaxWarning = 86,
        FutureWarning = 87,
        ImportWarning = 88,
        UnicodeWarning = 89,
        BytesWarning = 90,
        ResourceWarning = 91,
    }
    other {
        UnsupportedOperation = 19 => "UnsupportedOperation",
        SubprocessError = 25 => "SubprocessError",
        CalledProcessError = 26 => "CalledProcessError",
        TimeoutExpired = 27 => "TimeoutExpired",
        BadGzipFile = 28 => "BadGzipFile",
        ZlibError = 29 => "error",
        TomlDecodeError = 30 => "TOMLDecodeError",
        JsonDecodeError = 31 => "JSONDecodeError",
        CancelledError = 33 => "CancelledError",
        BadZipFile = 36 => "BadZipFile",
        UnidiffParseError = 52 => "UnidiffParseError",
        ShutilError = 64 => "Error",
        SameFileError = 65 => "SameFileError",
        SpecialFileError = 66 => "SpecialFileError",
        CsvError = 67 => "_csv.Error",
        GaiError = 68 => "socket.gaierror",
        BinasciiError = 69 => "binascii.Error",
        BinasciiIncomplete = 70 => "binascii.Incomplete",
        PackageNotFoundError = 71 => "PackageNotFoundError",
        HError = 72 => "socket.herror",
        LzmaError = 94 => "LZMAError",
    }
}

/// Resolves an unqualified exception name accepted by the checked frontend.
pub fn builtin_id(name: &str) -> Option<u32> {
    let kind = match name {
        "EnvironmentError" | "IOError" => Kind::OSError,
        _ => match boxed_builtin_kind(name)? {
            Kind::ChildProcessError | Kind::ProcessLookupError | Kind::BaseExceptionGroup => {
                return None;
            }
            kind => kind,
        },
    };
    Some(kind as u32)
}

/// Indices used by typed warning filters and by the runtime's -W option.
pub const WARNING_CATEGORIES: &[Kind] = &[
    Kind::Warning,
    Kind::ResourceWarning,
    Kind::RuntimeWarning,
    Kind::UserWarning,
    Kind::DeprecationWarning,
    Kind::PendingDeprecationWarning,
    Kind::SyntaxWarning,
    Kind::FutureWarning,
    Kind::ImportWarning,
    Kind::UnicodeWarning,
    Kind::BytesWarning,
    Kind::EncodingWarning,
];

impl Kind {
    /// Tests a builtin handler against this exception's class.
    pub fn matches(self, expected: Self) -> bool {
        match expected {
            Self::BaseException => true,
            Self::BaseExceptionGroup => {
                matches!(self, Self::BaseExceptionGroup | Self::ExceptionGroup)
            }
            Self::Warning => WARNING_CATEGORIES.contains(&self),
            Self::NameError => matches!(self, Self::NameError | Self::UnboundLocalError),
            Self::SyntaxError => matches!(
                self,
                Self::SyntaxError | Self::IndentationError | Self::TabError
            ),
            Self::IndentationError => matches!(self, Self::IndentationError | Self::TabError),
            Self::Exception => !matches!(
                self,
                Self::BaseException
                    | Self::BaseExceptionGroup
                    | Self::SystemExit
                    | Self::GeneratorExit
                    | Self::KeyboardInterrupt
                    | Self::CancelledError
            ),
            Self::ArithmeticError => matches!(
                self,
                Self::ArithmeticError
                    | Self::FloatingPointError
                    | Self::ZeroDivisionError
                    | Self::OverflowError
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
            Self::ImportError => matches!(
                self,
                Self::ImportError | Self::ModuleNotFoundError | Self::PackageNotFoundError
            ),
            Self::ModuleNotFoundError => {
                matches!(self, Self::ModuleNotFoundError | Self::PackageNotFoundError)
            }
            Self::SubprocessError => {
                matches!(
                    self,
                    Self::SubprocessError | Self::CalledProcessError | Self::TimeoutExpired
                )
            }
            Self::RuntimeError => matches!(
                self,
                Self::RuntimeError
                    | Self::NotImplementedError
                    | Self::RecursionError
                    | Self::PythonFinalizationError
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
                    | Self::ChildProcessError
                    | Self::ProcessLookupError
                    | Self::TimeoutError
                    | Self::GaiError
                    | Self::HError
                    | Self::BadGzipFile
                    | Self::ShutilError
                    | Self::SameFileError
                    | Self::SpecialFileError
            ),
            _ => self == expected,
        }
    }
}
