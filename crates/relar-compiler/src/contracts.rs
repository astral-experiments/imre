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
                    TextOperation::Split(_)
                    | TextOperation::Partition
                    | TextOperation::ShlexSplit => true,
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
                    | PathOperation::WithName
                    | PathOperation::Resolve
                    | PathOperation::WriteText
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
                ProcessOperation::Executable
                | ProcessOperation::Exit
                | ProcessOperation::RunToStderr
                | ProcessOperation::Run
                | ProcessOperation::RunTimeout
                | ProcessOperation::EnvironCopy
                | ProcessOperation::EnvironGet
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
            | Self::JsonLoad
            | Self::JsonLoads
            | Self::JsonGet { .. }
            | Self::JsonGetReceiver
            | Self::JsonAdd
            | Self::JsonAddFields(_, _)
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
            | Self::JsonIsInt
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
            | Self::ParseArgs(_)
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
            | Self::KeySet
            | Self::KeyIntersection
            | Self::NamespaceView
            | Self::ListAllI32
            | Self::ListDistinct
            | Self::ListExtendUnhinted
            | Self::Repr
            | Self::SetAdd
            | Self::SetDiscard
            | Self::SetIter
            | Self::SortDetach
            | Self::SortedRecordRows { .. }
            | Self::SortedStringKeys
            | Self::StringIter
            | Self::StringReverseIter
            | Self::TextClosed
            | Self::TupleCacheNew
            | Self::TupleCacheResult => Contract::new(false, Unknown),
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
                | "int_mod"
                | "int_lshift"
                | "int_rshift"
                | "relar_flush_stdout"
                | "relar_subprocess_run"
                | "relar_subprocess_run_timeout"
                | "relar_subprocess_run_timeout_int"
                | "relar_environ_get"
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
                | "relar_json_add"
                | "relar_json_get_receiver"
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
                | "relar_path_mkdir"
                | "relar_path_unlink"
                | "relar_path_rmdir"
                | "relar_path_replace"
                | "relar_path_absolute"
                | "relar_path_with_name"
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
                | "relar_list_get"
                | "relar_list_set"
                | "relar_list_get_index"
                | "relar_list_set_index"
                | "relar_list_borrow_index"
                | "relar_list_copy_index"
                | "relar_list_pop"
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
