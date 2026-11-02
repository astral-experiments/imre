//! Exception IDs shared by checked Python lowering and the native runtime.

macro_rules! exception_kinds {
    (
        checked_builtins { $($builtin:ident = $builtin_id:literal,)* }
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

        /// Resolves an unqualified exception name accepted by the checked frontend.
        pub fn builtin_id(name: &str) -> Option<u32> {
            match name {
                $(stringify!($builtin) => Some($builtin_id),)*
                _ => None,
            }
        }
    };
}

exception_kinds! {
    checked_builtins {
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
        FileNotFoundError = 20,
        PermissionError = 21,
        IsADirectoryError = 22,
        NotADirectoryError = 23,
        FileExistsError = 24,
        SystemExit = 32,
        ExceptionGroup = 34,
        StopIteration = 35,
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
        ReferenceError = 53,
        BufferError = 54,
        SyntaxError = 55,
        IndentationError = 56,
        GeneratorExit = 59,
        StopAsyncIteration = 60,
        KeyboardInterrupt = 61,
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
        ChildProcessError = 57 => "ChildProcessError",
        ProcessLookupError = 58 => "ProcessLookupError",
        ShutilError = 64 => "Error",
        SameFileError = 65 => "SameFileError",
        SpecialFileError = 66 => "SpecialFileError",
        CsvError = 67 => "_csv.Error",
        GaiError = 68 => "socket.gaierror",
        BinasciiError = 69 => "binascii.Error",
        BinasciiIncomplete = 70 => "binascii.Incomplete",
        PackageNotFoundError = 71 => "PackageNotFoundError",
    }
}

impl Kind {
    /// Tests a builtin handler against this exception's class.
    pub fn matches(self, expected: Self) -> bool {
        match expected {
            Self::BaseException => true,
            Self::Warning => matches!(
                self,
                Self::Warning
                    | Self::RuntimeWarning
                    | Self::DeprecationWarning
                    | Self::EncodingWarning
                    | Self::UserWarning
                    | Self::PendingDeprecationWarning
                    | Self::SyntaxWarning
                    | Self::FutureWarning
                    | Self::ImportWarning
                    | Self::UnicodeWarning
                    | Self::BytesWarning
                    | Self::ResourceWarning
            ),
            Self::NameError => matches!(self, Self::NameError | Self::UnboundLocalError),
            Self::SyntaxError => matches!(self, Self::SyntaxError | Self::IndentationError),
            Self::Exception => !matches!(
                self,
                Self::BaseException
                    | Self::SystemExit
                    | Self::GeneratorExit
                    | Self::KeyboardInterrupt
                    | Self::CancelledError
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
                    | Self::ChildProcessError
                    | Self::ProcessLookupError
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
