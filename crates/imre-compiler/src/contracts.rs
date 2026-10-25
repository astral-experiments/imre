//! Contracts for checked operations and the generic native call boundary.
//!
//! These facts describe an operation's own work, after its arguments have been
//! evaluated. Argument effects, call targets, alias proofs and fresh receivers
//! remain the responsibility of the analysis using the contract. In particular,
//! preserving lists does not imply freedom from I/O, allocation or errors.

use crate::ir::{
    Binary, Builtin, Expr, PathOperation, PosixPathOperation, ProcessOperation, TextOperation, Type,
};

/// Guarantees needed by list specialization and rooted heap borrows.
#[derive(Clone, Copy)]
enum HeapEffect {
    /// Preserves existing list contents/storage and heap ownership edges without
    /// calling compiled user code. Other state can change: a defaultdict lookup
    /// may insert a new entry without invalidating any existing owner or list.
    PreservesLists,
    /// May resize or reorder list storage, but preserves existing heap owners
    /// without calling compiled user code.
    PreservesOwners,
    /// Requires a separate proof before retaining container facts or borrows.
    Unknown,
}

/// Independent error and heap guarantees for one checked builtin.
pub(crate) struct Contract {
    pub may_raise: bool,
    heap: HeapEffect,
}

impl Contract {
    fn new(may_raise: bool, heap: HeapEffect) -> Self {
        Self { may_raise, heap }
    }

    /// Permits retaining list contents, lengths and buffer addresses.
    /// This is not a purity or speculation guarantee: the operation may raise or do I/O.
    pub(crate) fn preserves_list_storage(&self) -> bool {
        matches!(self.heap, HeapEffect::PreservesLists)
    }

    /// Preserves existing ownership edges used to root borrowed values.
    /// List storage and the addresses of individual slots may still change.
    pub(crate) fn preserves_owners(&self) -> bool {
        matches!(
            self.heap,
            HeapEffect::PreservesLists | HeapEffect::PreservesOwners
        )
    }
}

impl Builtin {
    /// Classifies every builtin explicitly; adding an operation requires a contract.
    /// Operand types refine conversions, but operand evaluation is never included.
    pub(crate) fn contract(&self, args: &[Expr]) -> Contract {
        use HeapEffect::{PreservesLists, PreservesOwners, Unknown};

        match self {
            Self::Repr => Contract::new(true, HeapEffect::PreservesLists),
            Self::Str
                if args
                    .first()
                    .is_some_and(|e| matches!(e.ty, Type::Exception | Type::NamedException(_))) =>
            {
                Contract::new(true, Unknown)
            }
            Self::ImportErrorReset => Contract::new(false, Unknown),
            Self::ImportErrorRejectReset => Contract::new(true, Unknown),
            Self::ImportErrorNew { .. }
            | Self::ImportErrorField(_)
            | Self::ExceptionFieldLen
            | Self::ExceptionFieldIndex => Contract::new(true, PreservesLists),
            Self::ExceptionTupleView | Self::ExceptionFieldRepr => {
                Contract::new(false, PreservesOwners)
            }
            Self::ExceptionRepr | Self::ExceptionMessage => Contract::new(true, Unknown),
            Self::ExceptionArgument(_) | Self::NamedException { .. } => {
                Contract::new(false, PreservesLists)
            }
            // Even a zero-argument call may mutate a retained native list or reenter Python.
            Self::ExtensionImport(_) | Self::ExtensionCall { .. } => Contract::new(true, Unknown),
            Self::Text(operation) => Contract::new(
                match operation {
                    TextOperation::SplitLineEnds
                    | TextOperation::Lower
                    | TextOperation::Capitalize
                    | TextOperation::Split(_)
                    | TextOperation::Partition
                    | TextOperation::Unquote
                    | TextOperation::ShlexSplit
                    | TextOperation::ShlexQuote => true,
                    TextOperation::IsSpace
                    | TextOperation::Strip(_)
                    | TextOperation::StartsWith
                    | TextOperation::EndsWith
                    | TextOperation::Find
                    | TextOperation::Dedent => false,
                },
                PreservesLists,
            ),
            Self::PosixPath(operation) => Contract::new(
                matches!(
                    operation,
                    PosixPathOperation::MixedJoin { .. }
                        | PosixPathOperation::Commonpath
                        | PosixPathOperation::Splitext
                        | PosixPathOperation::Normpath
                        | PosixPathOperation::Abspath
                ),
                PreservesLists,
            ),
            Self::Fnmatch(_) => Contract::new(true, PreservesLists),
            Self::Path(operation) => Contract::new(
                match operation {
                    PathOperation::ContextExit
                    | PathOperation::ReadText
                    | PathOperation::ReadBytes
                    | PathOperation::Absolute
                    | PathOperation::WithName
                    | PathOperation::WithStem
                    | PathOperation::WithSuffix
                    | PathOperation::Suffixes
                    | PathOperation::RelativeTo
                    | PathOperation::Match { .. }
                    | PathOperation::Resolve
                    | PathOperation::WriteText
                    | PathOperation::WriteUnion { .. }
                    | PathOperation::WriteTypeError
                    | PathOperation::WriteBytes
                    | PathOperation::Mkdir
                    | PathOperation::Unlink
                    | PathOperation::Rmdir
                    | PathOperation::Replace
                    | PathOperation::TemporaryDirectory
                    | PathOperation::CleanupTemporaryDirectory
                    | PathOperation::Iterdir
                    | PathOperation::Walk
                    | PathOperation::Glob { .. } => true,
                    PathOperation::ContextNew { .. }
                    | PathOperation::ContextEnter
                    | PathOperation::UnionText
                    | PathOperation::Scripts
                    | PathOperation::OsExists
                    | PathOperation::New
                    | PathOperation::PureNew(_)
                    | PathOperation::JoinObject
                    | PathOperation::Drive
                    | PathOperation::Root
                    | PathOperation::Anchor
                    | PathOperation::CountPart
                    | PathOperation::IsRelativeTo
                    | PathOperation::AsPosix
                    | PathOperation::Join
                    | PathOperation::Text
                    | PathOperation::Name
                    | PathOperation::Stem
                    | PathOperation::Suffix
                    | PathOperation::Parent
                    | PathOperation::IsAbsolute
                    | PathOperation::Exists
                    | PathOperation::IsFile
                    | PathOperation::IsDir => false,
                },
                PreservesLists,
            ),
            Self::Timed(operation) => Contract::new(
                matches!(
                    operation,
                    crate::ir::TimedOperation::New
                        | crate::ir::TimedOperation::Clock
                        | crate::ir::TimedOperation::Close
                        | crate::ir::TimedOperation::Flush
                        | crate::ir::TimedOperation::ComputeInt
                        | crate::ir::TimedOperation::ComputeFloat
                        | crate::ir::TimedOperation::FilesToDelete
                        | crate::ir::TimedOperation::ShouldRollover
                        | crate::ir::TimedOperation::DoRollover
                ),
                PreservesLists,
            ),
            Self::TimeSleep | Self::PercentScalar => Contract::new(true, PreservesLists),
            Self::Formatter(_) => Contract::new(true, PreservesLists),
            Self::LogRecord(_) => Contract::new(true, PreservesLists),
            Self::Logging(operation) => Contract::new(
                matches!(
                    operation,
                    crate::ir::LogOperation::Configure
                        | crate::ir::LogOperation::ConfigureName
                        | crate::ir::LogOperation::DictConfig
                        | crate::ir::LogOperation::DictConfigTyped
                        | crate::ir::LogOperation::FileConfig
                        | crate::ir::LogOperation::CheckLevel
                ),
                PreservesLists,
            ),
            Self::Process(operation) => match operation {
                ProcessOperation::DescriptorConstant(_) => Contract::new(false, PreservesLists),
                ProcessOperation::RunUntilComplete
                | ProcessOperation::CloseLoop
                | ProcessOperation::Gather { .. }
                | ProcessOperation::AsyncStatus { wait: true }
                | ProcessOperation::AsyncCommunicate
                | ProcessOperation::Await
                | ProcessOperation::RunLoop
                | ProcessOperation::JoinGroup
                | ProcessOperation::GeneratorNext
                | ProcessOperation::GeneratorYield
                | ProcessOperation::Acquire => Contract::new(true, Unknown),
                ProcessOperation::Executable
                | ProcessOperation::CheckOutput { .. }
                | ProcessOperation::RunToStderr
                | ProcessOperation::Run { .. }
                | ProcessOperation::Popen { .. }
                | ProcessOperation::Call { .. }
                | ProcessOperation::PopenPid
                | ProcessOperation::PopenPipe(_)
                | ProcessOperation::PopenArgs
                | ProcessOperation::PopenStatus { .. }
                | ProcessOperation::PopenExit
                | ProcessOperation::PopenCommunicate { .. }
                | ProcessOperation::PopenWait
                | ProcessOperation::PopenSignal
                | ProcessOperation::AsyncSpawn { .. }
                | ProcessOperation::AsyncStatus { wait: false }
                | ProcessOperation::Coroutine { .. }
                | ProcessOperation::IndirectCoroutine { .. }
                | ProcessOperation::TaskGroup
                | ProcessOperation::ContextNew { .. }
                | ProcessOperation::ContextEnter
                | ProcessOperation::ContextExit
                | ProcessOperation::ContextThrow
                | ProcessOperation::GeneratorTake
                | ProcessOperation::ContextYield
                | ProcessOperation::CreateTask
                | ProcessOperation::NewLoop
                | ProcessOperation::SetLoop
                | ProcessOperation::EnsureTask
                | ProcessOperation::LoopSignal
                | ProcessOperation::CancelTask
                | ProcessOperation::EnterGroup
                | ProcessOperation::AbortGroup
                | ProcessOperation::MemberCause
                | ProcessOperation::ExceptionAt
                | ProcessOperation::TaskResult
                | ProcessOperation::Semaphore
                | ProcessOperation::Release
                | ProcessOperation::EnvironCopy
                | ProcessOperation::EnvironGet
                | ProcessOperation::GetCwd
                | ProcessOperation::Fd(_)
                | ProcessOperation::OsErrorField(_)
                | ProcessOperation::Strerror
                | ProcessOperation::Which
                | ProcessOperation::RmTree
                | ProcessOperation::Copy(_)
                | ProcessOperation::CheckReturnCode
                | ProcessOperation::ExceptionView
                | ProcessOperation::ErrorField(_)
                | ProcessOperation::TimeoutField(_) => Contract::new(true, PreservesLists),
            },
            Self::RegexCapture { known, .. } => Contract::new(!known, PreservesLists),
            Self::Document(_) => Contract::new(true, Unknown),
            Self::Bytes => Contract::new(
                args.first().is_none_or(|arg| arg.ty != Type::Bytes),
                PreservesLists,
            ),
            Self::Int => Contract::new(
                args.first()
                    .is_none_or(|arg| !matches!(arg.ty, Type::Int | Type::Bool)),
                PreservesLists,
            ),
            Self::Float => Contract::new(
                args.first()
                    .is_none_or(|arg| !matches!(arg.ty, Type::Float | Type::Bool)),
                PreservesLists,
            ),
            Self::BytesGet
            | Self::Chr
            | Self::IntegerPower
            | Self::ModularPower
            | Self::NegativeIntegerPower
            | Self::Decode
            | Self::DecodeAscii
            | Self::UrlsafeB64encode
            | Self::HashNew
            | Self::DecodeSplitLines
            | Self::SplitLines
            | Self::SetAdd
            | Self::CounterUpdate
            | Self::KeySet
            | Self::KeyUnion
            | Self::KeyIntersection
            | Self::KeyDifference
            | Self::RegexGroupDict
            | Self::DictGet
            | Self::NamespaceWrap
            | Self::NamespaceDict
            | Self::NamespaceGet
            | Self::DictCopy
            | Self::DeepCopy
            | Self::RecordCopy
            | Self::JsonDecode(_)
            | Self::CsvReader { .. }
            | Self::CsvNext { .. }
            | Self::JsonEncode(_)
            | Self::JsonLoads
            | Self::JsonErrorField(_)
            | Self::HashRecord
            | Self::JsonGet { .. }
            | Self::JsonGetReceiver
            | Self::JsonAdd
            | Self::JsonEqual
            | Self::JsonAddFields(_, _)
            | Self::JsonDumps(_)
            | Self::JsonDump(_)
            | Self::JsonIndex
            | Self::JsonIter
            | Self::JsonLen
            | Self::JsonSelect(_)
            | Self::ListGet
            | Self::ListGetBounded { .. }
            | Self::CacheProbe(_)
            | Self::CacheInsert(_)
            | Self::ListGetI32
            | Self::ListSliceSum
            | Self::Slice
            | Self::StringFormat
            | Self::RegexSub(_)
            | Self::StringIterationList
            | Self::StringJoin
            | Self::StringReplace
            | Self::StringGet
            | Self::StringIntList
            | Self::TomlDecode(_) => Contract::new(true, PreservesLists),
            Self::Abs
            | Self::Bool
            | Self::DictContains
            | Self::DictGetDefault
            | Self::EnumLookup(_)
            | Self::DictSumValues
            | Self::DictTryGet(_)
            | Self::Encode
            | Self::CsvLineNumber
            | Self::Sha256Hex
            | Self::Sha256New
            | Self::StrDescriptor(_)
            | Self::NewPlainObject
            | Self::InspectSentinel
            | Self::HashAvailable
            | Self::Sha256Update
            | Self::Sha256Digest { .. }
            | Self::Sha256Copy
            | Self::ActiveException
            | Self::ExceptionRecord
            | Self::ExceptionCause
            | Self::ExceptionSuppressContext
            | Self::FloatFormat { .. }
            | Self::HexFormat { .. }
            | Self::GlobalGet(_)
            | Self::IntBounded { .. }
            | Self::IntSignAsFloat
            | Self::IsInstance(_)
            | Self::IsNone
            | Self::JsonIsInt
            | Self::Len
            | Self::JsonEmptyObject
            | Self::WallTime
            | Self::CacheNew
            | Self::CacheTable
            | Self::CacheStat(_)
            | Self::CacheParameter(_)
            | Self::CacheClear
            | Self::CallableContext
            | Self::ModuleCache(_)
            | Self::HashStr
            | Self::ListGetBoundedInBounds { .. }
            | Self::ListSum
            | Self::DirectoryContextView
            | Self::LiteralStringView
            | Self::RStrip
            | Self::RegexCompile(_)
            | Self::RegexSearch
            | Self::RegexMatchStart
            | Self::SequenceGetInBounds
            | Self::SetContains
            | Self::Split
            | Self::StringListFilter(_)
            | Self::StringListNonempty
            | Self::Str
            | Self::NominalIdentity
            | Self::ExceptionMatchesAny
            | Self::StrEnumMember { .. }
            | Self::ClassNameView
            | Self::StrEnumName
            | Self::StrEnumValue
            | Self::EnumString
            | Self::StringListContains
            | Self::StringContains
            | Self::BytesContains
            | Self::UnionInject
            | Self::UnionProject => Contract::new(false, PreservesLists),
            Self::Append | Self::SortByKeys => Contract::new(true, PreservesOwners),
            Self::ListReserveRange => Contract::new(false, PreservesOwners),
            Self::BinaryOpen(_)
            | Self::BinaryRead
            | Self::BinaryReadSize
            | Self::Ndiff
            | Self::StringListCopy
            | Self::StringListSortSuffix(_)
            | Self::BinaryWrite
            | Self::BytesValidateUtf8
            | Self::BytesContainsInt
            | Self::NamespaceDelete
            | Self::DictDelete
            | Self::DictPop
            | Self::DictTryPop(_)
            | Self::DictGatherBytesInt
            | Self::DictGatherStrInt
            | Self::FileReadline
            | Self::GzipOpen
            | Self::BytesIo
            | Self::BinarySeek
            | Self::BinaryTell
            | Self::ZipOpenFile
            | Self::ZipGetInfo
            | Self::ZipInfoList
            | Self::ZipMember
            | Self::ZipRead
            | Self::ZipTest
            | Self::ZipFilename
            | Self::ZipMinReadSize
            | Self::ZipField(_)
            | Self::ZipOpen
            | Self::ZipNamelist
            | Self::ZipClose
            | Self::Input
            | Self::ParseArgs { .. }
            | Self::ArgparseText { .. }
            | Self::Pop
            | Self::Print
            | Self::SortRestore
            | Self::StderrLine
            | Self::SysStreamWrite { .. }
            | Self::SysStreamFlush { .. }
            | Self::RaiseException
            | Self::SystemExitNew { .. }
            | Self::SystemExitCode
            | Self::SystemExitCodeEqual { .. }
            | Self::SysPlatform
            | Self::OsName
            | Self::SysOrigArgv
            | Self::SysArgv
            | Self::FileFileno
            | Self::TextClose
            | Self::TextEnter
            | Self::TextFlush
            | Self::TextOpen(_)
            | Self::TextRead
            | Self::TextWrite
            | Self::TomlKeys => Contract::new(true, Unknown),
            Self::Repeat
            | Self::CursorLengthHint
            | Self::Progression { .. }
            | Self::Tee
            | Self::Islice
            | Self::Cycle => Contract::new(true, PreservesLists),
            Self::BytesIter
            | Self::DefaultListDict
            | Self::CounterNew
            | Self::CounterTotal
            | Self::DictAsBase
            | Self::DictClear
            | Self::DictIter(_)
            | Self::ExceptionWithCause
            | Self::GlobalSet(_)
            | Self::ExactRecord(_)
            | Self::RecordView
            | Self::ListAllI32
            | Self::ListDistinct
            | Self::ListExtendUnhinted
            | Self::SetDiscard
            | Self::SetIter
            | Self::SortDetach
            | Self::SplitLinesIter
            | Self::StringIter
            | Self::StringReverseIter
            | Self::TextClosed => Contract::new(false, Unknown),
            Self::CounterMath { .. }
            | Self::CounterMostCommon
            | Self::SortedRecordRows { .. }
            | Self::SortedStringKeys
            | Self::TupleCacheNew
            | Self::TupleCacheResult => Contract::new(true, Unknown),
        }
    }
}

#[cfg(any(debug_assertions, test, feature = "verify-ir"))]
impl Builtin {
    /// Checks the representations consumed by specialized list and range lowerings.
    /// Dominance, aliasing and the truth of recorded bounds remain pass proofs.
    pub(crate) fn verify_specialization(
        &self,
        args: &[Expr],
        result: Type,
        types: &crate::types::Types,
    ) -> Result<(), &'static str> {
        use crate::ir::ExprKind;

        let signature = |params: &[Type], returns| {
            result == returns && args.iter().map(|arg| arg.ty).eq(params.iter().copied())
        };
        let bounds = |min, max| min <= max && min >= -(1_i64 << 62) && max < (1_i64 << 62);
        let index = |value: &Expr| match &value.kind {
            ExprKind::Local(_) => true,
            ExprKind::Int(value) => value.parse::<u64>().is_ok_and(|value| value < (1 << 62)),
            _ => false,
        };
        let valid = match self {
            Self::RecordCopy => matches!(result, Type::Class(_)) && signature(&[result], result),
            Self::ListGetI32 => signature(&[Type::ListInt, Type::Int], Type::Int),
            Self::ListGetBounded { min, max } | Self::ListGetBoundedInBounds { min, max } => {
                signature(&[Type::ListInt, Type::Int], Type::Int)
                    && bounds(*min, *max)
                    && (!matches!(self, Self::ListGetBoundedInBounds { .. })
                        || (matches!(args[0].kind, ExprKind::Local(_)) && index(&args[1])))
            }
            Self::SequenceGetInBounds => matches!(args, [source, cursor]
                if matches!(source.ty, Type::List(_))
                    && types.element(source.ty) == Some(result)
                    && matches!(source.kind, ExprKind::Local(_))
                    && cursor.ty == Type::Int && index(cursor)),
            Self::IntBounded { min, max } => {
                signature(&[Type::Int], Type::Int)
                    && bounds(*min, *max)
                    && matches!(args[0].kind, ExprKind::Local(_))
            }
            Self::ListAllI32 => signature(
                &[Type::ListInt, Type::Int, Type::Int, Type::Int],
                Type::Bool,
            ),
            Self::ListReserveRange => {
                signature(
                    &[Type::ListInt, Type::Int, Type::Int, Type::Int, Type::Int],
                    Type::None,
                ) && matches!(args[4].kind, ExprKind::Int(_))
                    && index(&args[4])
            }
            Self::ListDistinct => matches!(args, [left, right]
                if types.element(left.ty).is_some() && types.element(right.ty).is_some() && result == Type::Bool),
            Self::ListSum => signature(&[Type::ListInt, Type::Int], Type::Int),
            Self::IntSignAsFloat => signature(&[Type::Int], Type::Float),
            Self::SplitLinesIter => signature(&[Type::Str], Type::Iterator),
            Self::CallableContext => {
                matches!(args, [callable] if types.cached_function(callable.ty).is_some_and(|wrapper| wrapper.frame == result))
            }
            Self::CacheParameter(_) => {
                matches!(args, [wrapper] if types.cached_function(wrapper.ty).is_some())
                    && result == Type::Document(crate::ir::DocumentShape::Any)
            }
            Self::CacheNew | Self::ModuleCache(_) => {
                args.is_empty() && result.cache_table().is_some()
            }
            Self::CacheTable => matches!(args, [state] if state.ty.cache_table() == Some(result)),
            Self::CacheProbe(capacity) => {
                *capacity >= -1
                    && matches!(args, [state, key] if state.ty.cache_table().and_then(|t| types.mapping(t)).is_some_and(|(k, _)| k == key.ty))
                    && result == Type::Bool
            }
            Self::CacheInsert(capacity) => {
                *capacity > 0
                    && matches!(args, [state, key, value] if state.ty.cache_table().and_then(|t| types.mapping(t)) == Some((key.ty, value.ty)))
                    && result == Type::None
            }
            Self::CacheStat(field) => {
                matches!(field, 0 | 1 | 3)
                    && matches!(args, [state] if state.ty.cache_table().is_some())
                    && result == Type::Int
            }
            Self::CacheClear => {
                matches!(args, [state] if state.ty.cache_table().is_some()) && result == Type::None
            }
            Self::TupleCacheNew => args.is_empty() && matches!(result, Type::Tuple(_)),
            Self::TupleCacheResult => matches!(args, [value]
                if matches!(value.kind, ExprKind::Local(_)) && matches!(result, Type::Tuple(_)) && value.ty == result),
            _ => true,
        };
        if valid {
            Ok(())
        } else {
            Err("invalid specialized operation operands, result or bounds")
        }
    }
}

impl Binary {
    /// Conservative operation effect before operand-specific conversion checks.
    pub(crate) fn may_raise(self) -> bool {
        match self {
            Self::Divide
            | Self::FloorDivide
            | Self::Modulo
            | Self::Repeat
            | Self::LeftShift
            | Self::RightShift => true,
            Self::Add
            | Self::Subtract
            | Self::Multiply
            | Self::Concat
            | Self::BitAnd
            | Self::BitOr
            | Self::BitXor => false,
        }
    }

    /// Includes checked byte concatenation and conversion of mixed integer/float operands.
    pub(crate) fn may_raise_with_operands(self, left: Type, right: Type) -> bool {
        self.may_raise()
            || (matches!(self, Self::Concat) && matches!(left, Type::Str | Type::Bytes))
            || matches!(
                (left, right),
                (Type::Int, Type::Float) | (Type::Float, Type::Int)
            )
    }
}

/// A native symbol, its borrowed inputs, and the convention for consuming its result.
pub(crate) struct RuntimeCall<R, const N: usize> {
    pub(crate) symbol: &'static str,
    pub(crate) params: [RuntimeParam; N],
    pub(crate) result: R,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RuntimeParam {
    Value(Type),
    /// A decoded, nonnegative index. A tagged integer word also lowers to `i64`.
    NativeIndex,
}

const INT_LIST: RuntimeParam = RuntimeParam::Value(Type::ListInt);
const TAGGED_INT: RuntimeParam = RuntimeParam::Value(Type::Int);

/// A successful call transfers one integer word. Raw zero signals a pending error.
pub(crate) struct OwnedIntOrError;
/// The word borrows the rooted list element until consumption. Raw zero signals a pending error.
pub(crate) struct BorrowedIntOrError;
/// One signals success. Zero signals a pending error.
pub(crate) struct StatusOrError;

pub(crate) const LIST_GET: RuntimeCall<OwnedIntOrError, 2> = RuntimeCall {
    symbol: "imre_list_get",
    params: [INT_LIST, TAGGED_INT],
    result: OwnedIntOrError,
};
pub(crate) const LIST_GET_INDEX: RuntimeCall<OwnedIntOrError, 2> = RuntimeCall {
    symbol: "imre_list_get_index",
    params: [INT_LIST, RuntimeParam::NativeIndex],
    result: OwnedIntOrError,
};
pub(crate) const LIST_BORROW_INDEX: RuntimeCall<BorrowedIntOrError, 2> = RuntimeCall {
    symbol: "imre_list_borrow_index",
    params: [INT_LIST, RuntimeParam::NativeIndex],
    result: BorrowedIntOrError,
};
pub(crate) const BYTE_OUTPUT_APPEND: RuntimeCall<StatusOrError, 2> = RuntimeCall {
    symbol: "imre_byte_output_append",
    params: [RuntimeParam::Value(Type::ByteOutput), TAGGED_INT],
    result: StatusOrError,
};
pub(crate) const BYTE_OUTPUT_APPEND_PAIR: RuntimeCall<StatusOrError, 3> = RuntimeCall {
    symbol: "imre_byte_output_append_pair",
    params: [
        RuntimeParam::Value(Type::ByteOutput),
        TAGGED_INT,
        TAGGED_INT,
    ],
    result: StatusOrError,
};
pub(crate) const LIST_APPEND: RuntimeCall<StatusOrError, 2> = RuntimeCall {
    symbol: "imre_list_append",
    params: [INT_LIST, TAGGED_INT],
    result: StatusOrError,
};
pub(crate) const LIST_APPEND_PAIR: RuntimeCall<StatusOrError, 3> = RuntimeCall {
    symbol: "imre_list_append_pair",
    params: [INT_LIST, TAGGED_INT, TAGGED_INT],
    result: StatusOrError,
};
pub(crate) const LIST_POP: RuntimeCall<OwnedIntOrError, 1> = RuntimeCall {
    symbol: "imre_list_pop",
    params: [INT_LIST],
    result: OwnedIntOrError,
};
pub(crate) const LIST_SET: RuntimeCall<StatusOrError, 3> = RuntimeCall {
    symbol: "imre_list_set",
    params: [INT_LIST, TAGGED_INT, TAGGED_INT],
    result: StatusOrError,
};
pub(crate) const LIST_SET_INDEX: RuntimeCall<StatusOrError, 3> = RuntimeCall {
    symbol: "imre_list_set_index",
    params: [INT_LIST, RuntimeParam::NativeIndex, TAGGED_INT],
    result: StatusOrError,
};
pub(crate) const LIST_COPY_INDEX: RuntimeCall<StatusOrError, 3> = RuntimeCall {
    symbol: "imre_list_copy_index",
    params: [
        INT_LIST,
        RuntimeParam::NativeIndex,
        RuntimeParam::NativeIndex,
    ],
    result: StatusOrError,
};

/// Error checks for the emitter's generic native call helpers. Specialized
/// lowerings that emit checked instructions directly retain their own control flow.
/// This list follows the runtime's pending-error contract. Proven integer
/// arithmetic, ownership operations, lookup-with-default, lengths and hashing
/// remain infallible; allocation failure and invalid ABI use are fatal.
pub(crate) fn runtime_call_may_raise(name: &str) -> bool {
    name.starts_with("fn_")
        || matches!(
            name,
            "int_floordiv"
                | "imre_chr"
                | "imre_int_pow"
                | "imre_int_modpow"
                | "imre_int_negative_pow"
                | "imre_json_index"
                | "imre_exception_message"
                | "imre_import_error_reject_reset"
                | "imre_exception_repr"
                | "imre_json_iter"
                | "imre_json_len"
                | "imre_json_get_receiver"
                | "imre_json_add"
                | "int_mod"
                | "int_lshift"
                | "int_rshift"
                | "imre_flush_stdout"
                | "imre_subprocess_check_output"
                | "imre_subprocess_run"
                | "imre_subprocess_run_timeout"
                | "imre_subprocess_run_timeout_int"
                | "imre_environ_get"
                | "imre_getcwd"
                | "imre_shutil_which"
                | "imre_environ_copy"
                | "imre_subprocess_check"
                | "imre_subprocess_error_field"
                | "imre_print_int"
                | "imre_print_float"
                | "imre_log_file_new"
                | "imre_log_clock_new"
                | "imre_log_file_close"
                | "imre_log_file_flush"
                | "imre_log_file_compute_int"
                | "imre_log_file_compute_float"
                | "imre_log_file_deletions"
                | "imre_log_file_should_roll"
                | "imre_log_file_roll"
                | "imre_time_sleep"
                | "imre_str_percent_int"
                | "imre_str_percent_text"
                | "imre_formatter_format"
                | "imre_formatter_time"
                | "imre_formatter_message"
                | "imre_logrecord_message"
                | "imre_log_config"
                | "imre_log_dict_config"
                | "imre_log_typed_dict_config"
                | "imre_log_file_config"
                | "imre_log_config_named"
                | "imre_log_check_level"
                | "imre_stderr_line"
                | "imre_enum_lookup"
                | "imre_str_unquote"
                | "imre_string_iterable_list"
                | "imre_str_join"
                | "imre_str_replace"
                | "imre_print_str"
                | "imre_print_bytes"
                | "imre_bytes_count"
                | "imre_bytes_list"
                | "imre_bytes_contains_int"
                | "imre_byte_output_finish"
                | "imre_bytes_get"
                | "imre_bytes_get_index"
                | "imre_bytes_slice"
                | "imre_str_concat"
                | "imre_bytes_concat"
                | "imre_bytes_repeat"
                | "imre_counter_update"
                | "imre_key_union"
                | "imre_key_intersection"
                | "imre_key_difference"
                | "imre_regex_groupdict"
                | "imre_bytes_decode_splitlines"
                | "imre_str_splitlines"
                | "imre_str_splitlines_options"
                | "imre_exception_field_index"
                | "imre_exception_field_len"
                | "imre_bytes_decode"
                | "imre_bytes_decode_ascii"
                | "imre_fnmatch_translate"
                | "imre_fnmatch_translate_bytes"
                | "imre_urlsafe_b64encode"
                | "imre_hash_new"
                | "imre_toml_keys"
                | "imre_bytes_validate_utf8"
                | "imre_binary_open"
                | "imre_binary_read"
                | "imre_binary_read_size"
                | "imre_binary_readline"
                | "imre_gzip_open"
                | "imre_binary_write"
                | "imre_print_bool"
                | "imre_print_none"
                | "imre_input"
                | "imre_sys_argv"
                | "imre_sys_orig_argv"
                | "imre_path_read_bytes"
                | "imre_path_read_text"
                | "imre_path_write_text"
                | "imre_path_read_text_options"
                | "imre_path_write_text_options"
                | "imre_path_write_type_error"
                | "imre_path_write_bytes_union"
                | "imre_path_write_text_union"
                | "imre_path_write_bytes"
                | "imre_path_mkdir"
                | "imre_path_unlink"
                | "imre_path_rmdir"
                | "imre_path_replace"
                | "imre_path_absolute"
                | "imre_path_with_name"
                | "imre_path_change_extension"
                | "imre_path_relative_to"
                | "imre_path_suffixes"
                | "imre_path_resolve"
                | "imre_tempdir_new"
                | "imre_tempdir_cleanup"
                | "imre_path_next"
                | "imre_text_open"
                | "imre_text_enter"
                | "imre_text_read"
                | "imre_text_readline"
                | "imre_text_write"
                | "imre_text_flush"
                | "imre_text_close"
                | "imre_bytes_io"
                | "imre_binary_seek"
                | "imre_binary_tell"
                | "imre_zip_open_file"
                | "imre_zip_getinfo"
                | "imre_zip_infolist"
                | "imre_zip_member"
                | "imre_zip_read"
                | "imre_zip_test"
                | "imre_zip_min_read_size"
                | "imre_zip_open"
                | "imre_zip_namelist"
                | "imre_zip_close"
                | "imre_int_parse"
                | "imre_int_float"
                | "imre_float_int"
                | "imre_float_parse"
                | "imre_bytes_float"
                | "imre_int_true_divide"
                | "imre_assert_fail"
                | "imre_range_step_zero"
                | "imre_list_repeat"
                | "imre_list_slice"
                | "imre_list_slice_sum"
                | "imre_str_get"
                | "imre_str_lower"
                | "imre_str_capitalize"
                | "imre_str_slice"
                | "imre_str_int_list"
                | "imre_seq_get"
                | "imre_seq_set"
                | "imre_seq_get_index"
                | "imre_seq_set_index"
                | "imre_seq_pop"
                | "imre_seq_repeat"
                | "imre_str_repeat"
                | "imre_seq_slice"
                | "imre_str_list_copy"
                | "imre_record_shallow_copy"
                | "imre_dict_copy"
                | "imre_dict_get"
                | "imre_dict_gather_str_int"
                | "imre_dict_gather_bytes_int"
                | "imre_ndiff_deferred"
                | "imre_str_list_sorted_suffix"
                | "imre_dict_delete"
                | "imre_dict_pop"
                | "imre_cursor_length_hint"
                | "imre_mapping_iter_next"
                | "imre_exception_raise"
                | "imre_exception_reraise"
        )
}
