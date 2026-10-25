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
            // Even a zero-argument call may mutate a retained native list or reenter Python.
            Self::ExtensionImport(_) | Self::ExtensionCall { .. } => Contract::new(true, Unknown),
            Self::Text(operation) => Contract::new(
                match operation {
                    TextOperation::Split(_)
                    | TextOperation::Partition
                    | TextOperation::Unquote
                    | TextOperation::ShlexSplit => true,
                    TextOperation::Strip(_)
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
                        | PosixPathOperation::Splitext
                        | PosixPathOperation::Normpath
                        | PosixPathOperation::Abspath
                ),
                PreservesLists,
            ),
            Self::Path(operation) => Contract::new(
                match operation {
                    PathOperation::ContextExit
                    | PathOperation::ReadText
                    | PathOperation::ReadBytes
                    | PathOperation::Absolute
                    | PathOperation::WithName
                    | PathOperation::Resolve
                    | PathOperation::WriteText
                    | PathOperation::WriteBytes
                    | PathOperation::Mkdir
                    | PathOperation::Unlink
                    | PathOperation::Rmdir
                    | PathOperation::Replace
                    | PathOperation::TemporaryDirectory
                    | PathOperation::CleanupTemporaryDirectory
                    | PathOperation::Iterdir
                    | PathOperation::Next => true,
                    PathOperation::ContextNew { .. }
                    | PathOperation::ContextEnter
                    | PathOperation::UnionText
                    | PathOperation::Scripts
                    | PathOperation::OsExists
                    | PathOperation::New
                    | PathOperation::Join
                    | PathOperation::Text
                    | PathOperation::Name
                    | PathOperation::Stem
                    | PathOperation::Suffix
                    | PathOperation::Parent
                    | PathOperation::IsAbsolute
                    | PathOperation::Exists
                    | PathOperation::IsFile
                    | PathOperation::IsDir
                    | PathOperation::Glob => false,
                },
                PreservesLists,
            ),
            Self::Logging(operation) => Contract::new(
                matches!(operation, crate::ir::LogOperation::Configure),
                PreservesLists,
            ),
            Self::Process(operation) => match operation {
                ProcessOperation::RunUntilComplete
                | ProcessOperation::CloseLoop
                | ProcessOperation::Gather { .. }
                | ProcessOperation::AsyncStatus { wait: true }
                | ProcessOperation::AsyncCommunicate
                | ProcessOperation::Await
                | ProcessOperation::RunLoop
                | ProcessOperation::JoinGroup
                | ProcessOperation::Acquire => Contract::new(true, Unknown),
                ProcessOperation::Executable
                | ProcessOperation::CheckOutput
                | ProcessOperation::RunToStderr
                | ProcessOperation::Run { .. }
                | ProcessOperation::AsyncSpawn { .. }
                | ProcessOperation::AsyncStatus { wait: false }
                | ProcessOperation::Coroutine { .. }
                | ProcessOperation::IndirectCoroutine { .. }
                | ProcessOperation::TaskGroup
                | ProcessOperation::ContextNew { .. }
                | ProcessOperation::ContextEnter
                | ProcessOperation::ContextExit
                | ProcessOperation::ContextThrow
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
                | ProcessOperation::Which
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
            | Self::Decode
            | Self::DecodeSplitLines
            | Self::DictGet
            | Self::JsonDecode(_)
            | Self::CsvReader { .. }
            | Self::CsvNext { .. }
            | Self::JsonEncode(_)
            | Self::JsonLoads
            | Self::HashRecord
            | Self::JsonGet { .. }
            | Self::JsonGetReceiver
            | Self::JsonAdd
            | Self::JsonAddFields(_, _)
            | Self::JsonDumps(_)
            | Self::JsonDump(_)
            | Self::JsonIndex
            | Self::JsonIter
            | Self::JsonLen
            | Self::JsonSelect(_)
            | Self::ListGet
            | Self::ListGetBounded { .. }
            | Self::ListGetI32
            | Self::ListSliceSum
            | Self::Slice
            | Self::StringFormat
            | Self::RegexSub(_)
            | Self::StringJoin
            | Self::StringReplace
            | Self::StringGet
            | Self::StringIntList
            | Self::TomlDecode(_) => Contract::new(true, PreservesLists),
            Self::Abs
            | Self::Bool
            | Self::DictContains
            | Self::DictGetDefault
            | Self::CounterUpdate
            | Self::EnumLookup(_)
            | Self::DictSumValues
            | Self::DictTryGet(_)
            | Self::Encode
            | Self::CsvLineNumber
            | Self::Sha256Hex
            | Self::Sha256New
            | Self::Sha256Update
            | Self::Sha256Digest { .. }
            | Self::Sha256Copy
            | Self::ExceptionCause
            | Self::ExceptionMessage
            | Self::ExceptionSuppressContext
            | Self::FloatFormat { .. }
            | Self::GlobalGet(_)
            | Self::IntBounded { .. }
            | Self::IntSignAsFloat
            | Self::IsInstance(_)
            | Self::IsNone
            | Self::JsonIsInt
            | Self::Len
            | Self::JsonEmptyObject
            | Self::WallTime
            | Self::FunctionCache(_)
            | Self::HashStr
            | Self::ListGetBoundedInBounds { .. }
            | Self::ListSum
            | Self::DirectoryContextView
            | Self::LiteralStringView
            | Self::RStrip
            | Self::RegexCompile(_)
            | Self::RegexSearch
            | Self::RegexGroupDict
            | Self::RegexMatchStart
            | Self::SequenceGetInBounds
            | Self::SetContains
            | Self::Split
            | Self::SplitLines
            | Self::Ndiff
            | Self::StringListCopy
            | Self::StringListFilter(_)
            | Self::StringListSortSuffix(_)
            | Self::StringListNonempty
            | Self::Str
            | Self::NominalIdentity
            | Self::StrEnumMember { .. }
            | Self::StrEnumName
            | Self::StrEnumValue
            | Self::EnumString
            | Self::StringListContains
            | Self::StringContains
            | Self::UnionInject
            | Self::UnionProject => Contract::new(false, PreservesLists),
            Self::Append | Self::SortByKeys => Contract::new(true, PreservesOwners),
            Self::ListReserveRange => Contract::new(false, PreservesOwners),
            Self::BinaryOpen(_)
            | Self::BinaryRead
            | Self::BinaryReadSize
            | Self::BinaryWrite
            | Self::BytesValidateUtf8
            | Self::DictDelete
            | Self::DictGatherBytesInt
            | Self::DictGatherStrInt
            | Self::FileReadline
            | Self::GzipOpen
            | Self::ZipOpen
            | Self::ZipNamelist
            | Self::ZipClose
            | Self::Input
            | Self::ParseArgs(_)
            | Self::Pop
            | Self::Print
            | Self::SortRestore
            | Self::StderrLine
            | Self::SysStreamWrite { .. }
            | Self::SysStreamFlush { .. }
            | Self::SysExit
            | Self::SystemExitNew { .. }
            | Self::SystemExitCode
            | Self::SystemExitCodeEqual { .. }
            | Self::SysPlatform
            | Self::SysArgv
            | Self::TextClose
            | Self::TextEnter
            | Self::TextFlush
            | Self::TextOpen(_)
            | Self::TextRead
            | Self::TextWrite
            | Self::TomlKeys => Contract::new(true, Unknown),
            Self::BytesIter
            | Self::DefaultListDict
            | Self::CounterNew
            | Self::CounterTotal
            | Self::DictAsBase
            | Self::DictIter(_)
            | Self::ExceptionWithCause
            | Self::GlobalSet(_)
            | Self::KeyUnion
            | Self::KeySet
            | Self::KeyIntersection
            | Self::ExactRecord(_)
            | Self::RecordView
            | Self::ListAllI32
            | Self::ListDistinct
            | Self::ListExtendUnhinted
            | Self::Repr
            | Self::SetAdd
            | Self::SetDiscard
            | Self::SetIter
            | Self::SortDetach
            | Self::SplitLinesIter
            | Self::StringIter
            | Self::StringReverseIter
            | Self::TextClosed => Contract::new(false, Unknown),
            Self::SortedRecordRows { .. }
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
            || (matches!(self, Self::Concat) && left == Type::Bytes)
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
                | "imre_json_index"
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
                | "imre_log_config"
                | "imre_stderr_line"
                | "imre_enum_lookup"
                | "imre_str_unquote"
                | "imre_str_join"
                | "imre_str_replace"
                | "imre_print_str"
                | "imre_print_bytes"
                | "imre_bytes_count"
                | "imre_bytes_list"
                | "imre_byte_output_finish"
                | "imre_bytes_get"
                | "imre_bytes_get_index"
                | "imre_bytes_slice"
                | "imre_bytes_concat"
                | "imre_bytes_repeat"
                | "imre_bytes_decode_splitlines"
                | "imre_bytes_decode"
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
                | "imre_path_read_bytes"
                | "imre_path_read_text"
                | "imre_path_write_text"
                | "imre_path_write_bytes"
                | "imre_path_mkdir"
                | "imre_path_unlink"
                | "imre_path_rmdir"
                | "imre_path_replace"
                | "imre_path_absolute"
                | "imre_path_with_name"
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
                | "imre_dict_get"
                | "imre_dict_gather_str_int"
                | "imre_dict_gather_bytes_int"
                | "imre_dict_delete"
                | "imre_mapping_iter_next"
                | "imre_exception_raise"
                | "imre_exception_reraise"
        )
}
