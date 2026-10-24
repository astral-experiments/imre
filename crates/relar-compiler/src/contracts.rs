//! Contracts for checked operations and the generic native call boundary.
//!
//! These facts describe an operation's own work, after its arguments have been
//! evaluated. Argument effects, call targets, alias proofs and fresh receivers
//! remain the responsibility of the analysis using the contract. In particular,
//! preserving lists does not imply freedom from I/O, allocation or errors.

use crate::ir::{Binary, Builtin, Expr, PathOperation, ProcessOperation, TextOperation, Type};

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
            Self::Text(operation) => Contract::new(
                match operation {
                    TextOperation::Split(_) | TextOperation::Partition => true,
                    TextOperation::Strip(_)
                    | TextOperation::StartsWith
                    | TextOperation::EndsWith
                    | TextOperation::Find
                    | TextOperation::Unquote
                    | TextOperation::Dedent => false,
                },
                PreservesLists,
            ),
            Self::Path(operation) => Contract::new(
                match operation {
                    PathOperation::ReadText
                    | PathOperation::Absolute
                    | PathOperation::Resolve
                    | PathOperation::WriteText
                    | PathOperation::WithName
                    | PathOperation::ReadBytes
                    | PathOperation::WriteBytes
                    | PathOperation::Mkdir
                    | PathOperation::Unlink
                    | PathOperation::Rmdir
                    | PathOperation::Replace
                    | PathOperation::TemporaryDirectory
                    | PathOperation::CleanupTemporaryDirectory
                    | PathOperation::Iterdir
                    | PathOperation::Next => true,
                    PathOperation::New
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
            Self::Process(operation) => match operation {
                ProcessOperation::Run
                | ProcessOperation::RunTimeout
                | ProcessOperation::EnvironCopy
                | ProcessOperation::EnvironGet
                | ProcessOperation::GetCwd
                | ProcessOperation::CheckReturnCode
                | ProcessOperation::ExceptionView
                | ProcessOperation::ErrorField(_)
                | ProcessOperation::TimeoutField(_) => Contract::new(true, PreservesLists),
            },
            Self::RegexCapture { known, .. } => Contract::new(!known, PreservesLists),
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
            | Self::DictGet
            | Self::JsonDecode(_)
            | Self::JsonEncode(_)
            | Self::JsonDumps(_)
            | Self::JsonDump(_)
            | Self::JsonLoads
            | Self::JsonIndex
            | Self::JsonIter
            | Self::JsonLen
            | Self::JsonGetReceiver
            | Self::JsonGet(_)
            | Self::JsonAdd
            | Self::JsonIsInt
            | Self::JsonSelect(_)
            | Self::ListGet
            | Self::ListGetBounded { .. }
            | Self::ListGetI32
            | Self::ListSliceSum
            | Self::Slice
            | Self::StringFormat
            | Self::StringGet
            | Self::StringIntList
            | Self::TomlDecode(_) => Contract::new(true, PreservesLists),
            Self::Abs
            | Self::Bool
            | Self::DictContains
            | Self::DictGetDefault
            | Self::DictTryGet(_)
            | Self::Encode
            | Self::ExceptionCause
            | Self::ExceptionMessage
            | Self::ExceptionSuppressContext
            | Self::FloatFormat { .. }
            | Self::GlobalGet(_)
            | Self::IntBounded { .. }
            | Self::IntSignAsFloat
            | Self::IsInstance(_)
            | Self::IsNone
            | Self::Len
            | Self::ListGetBoundedInBounds { .. }
            | Self::ListSum
            | Self::LiteralStringView
            | Self::RStrip
            | Self::RegexCompile(_)
            | Self::RegexSearch
            | Self::SequenceGetInBounds
            | Self::SetContains
            | Self::Split
            | Self::SplitLines
            | Self::Str
            | Self::StrEnumIdentity
            | Self::StrEnumMember { .. }
            | Self::StrEnumName
            | Self::StrEnumValue
            | Self::StringJoin
            | Self::StringListContains
            | Self::StringReplace
            | Self::UnionInject
            | Self::UnionProject => Contract::new(false, PreservesLists),
            Self::Append | Self::ListReserveRange | Self::SortByKeys => {
                Contract::new(false, PreservesOwners)
            }
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
            | Self::Input
            | Self::ParseArgs
            | Self::Pop
            | Self::Print
            | Self::SortRestore
            | Self::StderrLine
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
            | Self::DictAsBase
            | Self::DictIter(_)
            | Self::ExceptionWithCause
            | Self::GlobalSet(_)
            | Self::KeyUnion
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
pub(crate) struct InfallibleVoid;
/// One signals success. Zero signals a pending error.
pub(crate) struct StatusOrError;

pub(crate) const LIST_GET: RuntimeCall<OwnedIntOrError, 2> = RuntimeCall {
    symbol: "relar_list_get",
    params: [INT_LIST, TAGGED_INT],
    result: OwnedIntOrError,
};
pub(crate) const LIST_GET_INDEX: RuntimeCall<OwnedIntOrError, 2> = RuntimeCall {
    symbol: "relar_list_get_index",
    params: [INT_LIST, RuntimeParam::NativeIndex],
    result: OwnedIntOrError,
};
pub(crate) const LIST_BORROW_INDEX: RuntimeCall<BorrowedIntOrError, 2> = RuntimeCall {
    symbol: "relar_list_borrow_index",
    params: [INT_LIST, RuntimeParam::NativeIndex],
    result: BorrowedIntOrError,
};
pub(crate) const LIST_APPEND: RuntimeCall<InfallibleVoid, 2> = RuntimeCall {
    symbol: "relar_list_append",
    params: [INT_LIST, TAGGED_INT],
    result: InfallibleVoid,
};
pub(crate) const LIST_POP: RuntimeCall<OwnedIntOrError, 1> = RuntimeCall {
    symbol: "relar_list_pop",
    params: [INT_LIST],
    result: OwnedIntOrError,
};
pub(crate) const LIST_SET: RuntimeCall<StatusOrError, 3> = RuntimeCall {
    symbol: "relar_list_set",
    params: [INT_LIST, TAGGED_INT, TAGGED_INT],
    result: StatusOrError,
};
pub(crate) const LIST_SET_INDEX: RuntimeCall<StatusOrError, 3> = RuntimeCall {
    symbol: "relar_list_set_index",
    params: [INT_LIST, RuntimeParam::NativeIndex, TAGGED_INT],
    result: StatusOrError,
};
pub(crate) const LIST_COPY_INDEX: RuntimeCall<StatusOrError, 3> = RuntimeCall {
    symbol: "relar_list_copy_index",
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
                | "relar_json_index"
                | "relar_json_iter"
                | "relar_json_len"
                | "relar_json_get_receiver"
                | "relar_json_add"
                | "int_mod"
                | "int_lshift"
                | "int_rshift"
                | "relar_flush_stdout"
                | "relar_subprocess_run"
                | "relar_subprocess_run_timeout"
                | "relar_subprocess_run_timeout_int"
                | "relar_environ_get"
                | "relar_getcwd"
                | "relar_environ_copy"
                | "relar_subprocess_check"
                | "relar_subprocess_error_field"
                | "relar_print_int"
                | "relar_print_float"
                | "relar_stderr_line"
                | "relar_print_str"
                | "relar_print_bytes"
                | "relar_bytes_count"
                | "relar_bytes_list"
                | "relar_bytes_get"
                | "relar_bytes_get_index"
                | "relar_bytes_slice"
                | "relar_bytes_concat"
                | "relar_bytes_repeat"
                | "relar_bytes_decode"
                | "relar_toml_keys"
                | "relar_bytes_validate_utf8"
                | "relar_binary_open"
                | "relar_binary_read"
                | "relar_binary_read_size"
                | "relar_binary_readline"
                | "relar_gzip_open"
                | "relar_binary_write"
                | "relar_print_bool"
                | "relar_print_none"
                | "relar_input"
                | "relar_sys_argv"
                | "relar_path_read_text"
                | "relar_path_write_text"
                | "relar_path_with_name"
                | "relar_path_read_bytes"
                | "relar_path_write_bytes"
                | "relar_path_mkdir"
                | "relar_path_unlink"
                | "relar_path_rmdir"
                | "relar_path_replace"
                | "relar_path_absolute"
                | "relar_path_resolve"
                | "relar_tempdir_new"
                | "relar_tempdir_cleanup"
                | "relar_path_next"
                | "relar_text_open"
                | "relar_text_enter"
                | "relar_text_read"
                | "relar_text_readline"
                | "relar_text_write"
                | "relar_text_flush"
                | "relar_text_close"
                | "relar_int_parse"
                | "relar_int_float"
                | "relar_float_int"
                | "relar_float_parse"
                | "relar_bytes_float"
                | "relar_int_true_divide"
                | "relar_assert_fail"
                | "relar_range_step_zero"
                | "relar_list_repeat"
                | "relar_list_slice"
                | "relar_list_slice_sum"
                | "relar_str_get"
                | "relar_str_slice"
                | "relar_str_int_list"
                | "relar_seq_get"
                | "relar_seq_set"
                | "relar_seq_get_index"
                | "relar_seq_set_index"
                | "relar_seq_pop"
                | "relar_seq_repeat"
                | "relar_seq_slice"
                | "relar_dict_get"
                | "relar_dict_gather_str_int"
                | "relar_dict_gather_bytes_int"
                | "relar_dict_delete"
                | "relar_mapping_iter_next"
                | "relar_exception_raise"
                | "relar_exception_reraise"
        )
}
