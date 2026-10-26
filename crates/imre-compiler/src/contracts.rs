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
            Self::Bool => Contract::new(args.first().is_some_and(|e| matches!(e.ty, Type::ObjectValue | Type::List(_) | Type::Union(_) | Type::Optional(_))), PreservesLists),
            Self::Len => Contract::new(
                args.first().is_some_and(|e| matches!(e.ty, Type::List(_))),
                PreservesLists,
            ),
            Self::StringListNonempty | Self::StringListContains | Self::StringListFilter(_) => {
                Contract::new(true, PreservesLists)
            }
            Self::ListReverse => Contract::new(args.first().is_some_and(|e| matches!(e.ty, Type::List(_))), Unknown),
            Self::ListExtendUnhinted | Self::SortDetach => Contract::new(true, Unknown),
            Self::FunctionWrappedSet => Contract::new(false, Unknown),
            Self::FunctionWrappedGet => Contract::new(true, PreservesLists),
            Self::Repr => Contract::new(true, Unknown),
            Self::DefaultRecordRepr => Contract::new(true, PreservesLists),
            Self::Str
                if args
                    .first()
                    .is_some_and(|e| matches!(e.ty, Type::Bytes | Type::ImportException)) =>
            {
                Contract::new(true, PreservesLists)
            }
            Self::Str
                if args
                    .first()
                    .is_some_and(|e| matches!(e.ty, Type::Exception | Type::NamedException(_) | Type::ObjectValue)) =>
            {
                Contract::new(true, Unknown)
            }
            Self::Str if args.first().is_some_and(|e| e.ty == Type::Int) => {
                Contract::new(true, PreservesOwners)
            }
            Self::Str => Contract::new(true, PreservesLists),
            Self::Linecache(..) | Self::CachePath(_) => Contract::new(true, Unknown),
            Self::Calendar(op) => Contract::new(
                matches!(
                    op,
                    crate::ir::CalendarOp::Date
                        | crate::ir::CalendarOp::DateTime
                        | crate::ir::CalendarOp::Combine
                        | crate::ir::CalendarOp::Delta
                        | crate::ir::CalendarOp::Zone
                        | crate::ir::CalendarOp::Now
                        | crate::ir::CalendarOp::EmailParseDate
                        | crate::ir::CalendarOp::EmailFormatDateTime
                        | crate::ir::CalendarOp::EmailFormatDate
                        | crate::ir::CalendarOp::Shift { .. }
                        | crate::ir::CalendarOp::Negate
                ),
                PreservesOwners,
            ),
            Self::ImportErrorReset => Contract::new(false, Unknown),
            Self::ImportErrorRejectReset => Contract::new(true, Unknown),
            Self::ImportErrorNew { .. }
            | Self::ImportErrorField(_)
            | Self::ExceptionArgs
            | Self::ExceptionFieldLen
            | Self::ExceptionFieldIndex
            | Self::ExceptionFormatterResult(_) => Contract::new(true, PreservesLists),
            Self::MappingObjectGet => Contract::new(true, PreservesOwners),
            Self::BoxValue
            | Self::ExceptionTupleView
            | Self::ObjectRecordProject(_)
            | Self::ObjectRecordIdentity { .. }
            | Self::ExceptionTupleIdentity
            | Self::ExceptionKeywordSuggestion
            | Self::ExceptionRecordFields => Contract::new(false, PreservesOwners),
            // Formatting a projected task-group member may call its own __repr__.
            Self::ExceptionFieldRepr
            | Self::ExceptionRepr
            | Self::ExceptionMessage
            | Self::Logging(
                crate::ir::LogOperation::PrintActiveException
                | crate::ir::LogOperation::DictConfig
                | crate::ir::LogOperation::DictConfigTyped
                | crate::ir::LogOperation::FileConfig,
            ) => {
                Contract::new(true, Unknown)
            }
            Self::SystemExitFromTuple
            | Self::ExceptionArgument(_)
            | Self::NamedException { .. } => Contract::new(false, PreservesLists),
            // Even a zero-argument call may mutate a retained native list or reenter Python.
            Self::ExtensionImport(_) | Self::ExtensionCall { .. } => Contract::new(true, Unknown),
            Self::SequenceTakeInBounds => Contract::new(false, Unknown),
            Self::Text(operation) => Contract::new(
                match operation {
                    TextOperation::SplitLineEnds
                    | TextOperation::ExpandTabs
                    | TextOperation::Lower
                    | TextOperation::Upper
                    | TextOperation::Capitalize
                    | TextOperation::Split(_)
                    | TextOperation::Partition
                    | TextOperation::Translate
                    | TextOperation::UrlSplit
                    | TextOperation::UrlProperty(_)
                    | TextOperation::UrlGetUrl
                    | TextOperation::UrlTranscode { .. }
                    | TextOperation::Unquote
                    | TextOperation::ShlexSplit
                    | TextOperation::ShlexQuote => true,
                    TextOperation::Predicate(_)
                    | TextOperation::Strip(_)
                    | TextOperation::StartsWith
                    | TextOperation::EndsWith
                    | TextOperation::Find
                    | TextOperation::Dedent => false,
                },
                PreservesLists,
            ),
            Self::PosixPath(_) => Contract::new(true, PreservesLists),
            Self::DictContains
            | Self::ChainMapContains
            | Self::DictGetDefault
            | Self::DictTryGet(_)
            | Self::SetContains => Contract::new(true, PreservesLists),
            Self::Socket(op) => Contract::new(
                !matches!(
                    op,
                    crate::ir::SocketOp::Constant(_)
                        | crate::ir::SocketOp::Fileno
                        | crate::ir::SocketOp::GetBlocking
                ),
                PreservesLists,
            ),
            Self::Fnmatch(_) => Contract::new(true, PreservesLists),
            Self::Path(operation) => Contract::new(
                match operation {
                    PathOperation::ContextExit
                    | PathOperation::Join
                    | PathOperation::JoinObject
                    | PathOperation::Parent
                    | PathOperation::IsRelativeTo
                    | PathOperation::AsPosix
                    | PathOperation::ReadText
                    | PathOperation::OpenTextOutput
                    | PathOperation::ReadBytes
                    | PathOperation::Readlink
                    | PathOperation::Link(_)
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
                    | PathOperation::Stat { .. }
                    | PathOperation::CheckStatPath
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
                    | PathOperation::Drive
                    | PathOperation::Root
                    | PathOperation::Anchor
                    | PathOperation::CountPart
                    | PathOperation::Text
                    | PathOperation::Name
                    | PathOperation::Stem
                    | PathOperation::Suffix
                    | PathOperation::IsAbsolute
                    | PathOperation::Info
                    | PathOperation::InfoRepr
                    | PathOperation::Query(_) => false,
                    PathOperation::StatTime(index) => *index == 3,
                    PathOperation::StatExtraInt(index) => (6..=8).contains(index),
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
            Self::ClockParts(_) | Self::TimeSleep
            | Self::PercentFormat
            | Self::WarningFilter { .. }
            | Self::WarningFormat { .. }
            | Self::WarningWrite { .. }
            | Self::WarningReset => Contract::new(true, PreservesLists),
            Self::Formatter(_) => Contract::new(true, PreservesLists),
            Self::LogRecord(_) => Contract::new(true, PreservesLists),
            Self::Patch(operation) => Contract::new(
                matches!(operation, crate::ir::PatchOperation::Parse),
                PreservesLists,
            ),
            Self::Logging(operation) => Contract::new(
                matches!(
                    operation,
                    crate::ir::LogOperation::CheckLevel
                        | crate::ir::LogOperation::WriteStream
                        | crate::ir::LogOperation::FlushStream
                        | crate::ir::LogOperation::LockAcquire
                        | crate::ir::LogOperation::LockAcquireInt
                        | crate::ir::LogOperation::LockEnter
                        | crate::ir::LogOperation::LockExit
                        | crate::ir::LogOperation::LockRelease
                        | crate::ir::LogOperation::InvalidLevel
                        | crate::ir::LogOperation::NewManager
                        | crate::ir::LogOperation::Manager
                        | crate::ir::LogOperation::Enabled
                ),
                PreservesLists,
            ),
            Self::Process(operation) => match operation {
                ProcessOperation::DescriptorConstant(_)
                | ProcessOperation::SelectorClass(_)
                | ProcessOperation::SetSelector
                | ProcessOperation::PopenSetStatus => Contract::new(false, PreservesLists),
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
                | ProcessOperation::Acquire
                | ProcessOperation::PopenSetPoll { .. }
                | ProcessOperation::PopenDeletePoll
                | ProcessOperation::PopenStatus { poll: true }
                | ProcessOperation::PopenSignal
                | ProcessOperation::ShellOutput { .. }
                | ProcessOperation::CheckOutput { .. }
                | ProcessOperation::RunToStderr
                | ProcessOperation::Run { .. }
                | ProcessOperation::Popen { .. }
                | ProcessOperation::PopenCleanup
                | ProcessOperation::Call { .. }
                | ProcessOperation::AsyncSpawn { .. } => Contract::new(true, Unknown),
                ProcessOperation::Executable
                | ProcessOperation::PopenPid
                | ProcessOperation::PopenActive
                | ProcessOperation::PopenIdentity
                | ProcessOperation::PopenPipe(_)
                | ProcessOperation::PopenArgs
                | ProcessOperation::PopenStatus { poll: false }
                | ProcessOperation::PopenExit
                | ProcessOperation::PopenCommunicate { .. }
                | ProcessOperation::PopenWait
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
                | ProcessOperation::Credential(_)
                | ProcessOperation::CredentialId { .. }
                | ProcessOperation::Readiness(_)
                | ProcessOperation::OsErrorField(_)
                | ProcessOperation::Strerror
                | ProcessOperation::QuoteCommandArgument
                | ProcessOperation::Which
                | ProcessOperation::RmTree
                | ProcessOperation::CopyTree(_)
                | ProcessOperation::Move(_)
                | ProcessOperation::DestInSrc
                | ProcessOperation::IgnorePatterns
                | ProcessOperation::TreeErrorArgs
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
            | Self::WrapperAttributes(_)
            | Self::Chr
            | Self::Ord
            | Self::Random(_)
            | Self::IntegerPower
            | Self::ModularPower
            | Self::NegativeIntegerPower
            | Self::Decode
            | Self::DecodeAscii
            | Self::UrlsafeB64encode
            | Self::CodecLookup
            | Self::StrEncodeOptions
            | Self::CodecEncode
            | Self::CodecDecode
            | Self::BytesIoValue
            | Self::CodecWriter(_)
            | Self::HashNew
            | Self::HashAvailable
            | Self::Sha1Hex
            | Self::Sha1New
            | Self::Sha256Hex
            | Self::Sha256New
            | Self::HashName
            | Self::HashDigest { .. }
            | Self::HashCopy
            | Self::DecodeSplitLines
            | Self::SplitLines
            | Self::FreezeStrings
            | Self::SetAdd
            | Self::SetDiscard
            | Self::CounterUpdate
            | Self::DictUpdate
            | Self::KeySet
            | Self::KeyUnion
            | Self::KeyIntersection
            | Self::KeyDifference
            | Self::RegexGroupDict
            | Self::RegexFindIter
            | Self::Errno(_)
            | Self::DictGet
            | Self::ModuleParentPrefix
            | Self::ModuleCacheFromOrigin
            | Self::NamespaceWrap
            | Self::NamespaceDict
            | Self::NamespaceGet
            | Self::DictCopy
            | Self::DictFromKeysTarget
            | Self::DeepCopy
            | Self::RecordCopy
            | Self::JsonDecode(_)
            | Self::CsvReader { .. }
            | Self::FormatCursor { .. }
            | Self::CsvNext { .. }
            | Self::JsonEncode(_)
            | Self::JsonLoads
            | Self::JsonEmptyObject
            | Self::JsonDecoderDecode
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
            | Self::TupleGet
            | Self::ListGet
            | Self::ListGetBounded { .. }
            | Self::CacheProbe(_)
            | Self::CacheInsert(_)
            | Self::EmptyPartial
            | Self::ListGetI32
            | Self::ListSliceSum
            | Self::Slice
            | Self::StringFormat
            | Self::RegexSub(_)
            | Self::UnittestCompilePattern
            | Self::StringIterationList
            | Self::StringJoin
            | Self::JoinBufferFinish
            | Self::JoinBufferNew
            | Self::StringReplace
            | Self::StringGet
            | Self::StringIntList
            | Self::EnumLookup(_)
            | Self::TomlDecode(_) => Contract::new(true, PreservesLists),
            Self::Abs
            | Self::DefaultFactoryEnabled
            | Self::DictSumValues
            | Self::Encode
            | Self::CsvLineNumber
            | Self::CodecName
            | Self::StrDescriptor(_)
            | Self::Placeholder(_)
            | Self::NewPlainObject
            | Self::InspectSentinel
            | Self::JsonDecoderNew
            | Self::JsonDecoderMemo
            | Self::HashUpdate
            | Self::HashDigestSize
            | Self::ActiveException
            | Self::SysIntern
            | Self::SysIsInterned
            | Self::ExceptionRecord
            | Self::ExceptionCause
            // A fresh exception retains its cause without replacing any existing owner.
            | Self::ExceptionWithCause
            | Self::ExceptionOrigin(_)
            | Self::ExceptionSuppressContext
            | Self::FloatFormat { .. }
            | Self::HexFormat { .. }
            | Self::GlobalGet(_)
            | Self::IntBounded { .. }
            | Self::CanElideMapperFrames(_)
            | Self::SysGetRecursionLimit
            | Self::GeneratorFrameLeave
            | Self::IntSignAsFloat
            | Self::IsInstance(_)
            | Self::IsBuiltinSubclass
            | Self::IsNone
            | Self::MockCallList { .. }
            | Self::JsonIsInt
            | Self::WallTime
            | Self::CacheNew
            | Self::CacheTable
            | Self::CacheStat(_)
            | Self::CacheParameter(_)
            | Self::CacheClear
            | Self::CallableContext
            | Self::ModuleCache(_)
            | Self::HashStr
            | Self::SequenceGetInBounds
            | Self::ListGetBoundedInBounds { .. }
            | Self::ListSum
            | Self::DirectoryContextView
            | Self::TextStreamView
            | Self::LiteralStringView
            | Self::RStrip
            | Self::RegexCompile { .. }
            | Self::RegexSpelling
            | Self::RegexSearch
            | Self::RegexMatchStart
            | Self::Split
            | Self::SourceModule
            | Self::SourceModuleAttribute { .. }
            | Self::NominalIdentity
            | Self::ExceptionMatchesAny
            | Self::StrEnumMember { .. }
            | Self::ForwardRefField(_)
            | Self::BoxAnnotation(_)
            | Self::AnnotationMember(_)
            | Self::AnnotationIs
            | Self::StaticFunction(_)
            | Self::NewForwardRef
            | Self::ClassNameView
            | Self::ClassNamePart { .. }
            | Self::IntEnumValue
            | Self::HttpEnumPhrase
            | Self::HttpEnumDescription
            | Self::StrEnumName
            | Self::StrEnumValue
            | Self::EnumString
            | Self::IntListContains
            | Self::StructListContains
            | Self::DequeContains
            | Self::StringContains
            | Self::BytesContains
            | Self::UnionInject
            | Self::UnionProject => Contract::new(false, PreservesLists),
            Self::Append | Self::JoinBufferAppend | Self::SortByKeys => {
                Contract::new(true, PreservesOwners)
            }
            Self::SortByComparator => Contract::new(true, Unknown),
            Self::ListReserveRange => Contract::new(false, PreservesOwners),
            Self::AnnotationRef
            | Self::BinaryOpen(_)
            | Self::BinaryRead
            | Self::BinaryReadSize
            | Self::Ndiff
            | Self::StringListCopy
            | Self::JoinedSplitLines
            | Self::StringListSortSuffix(_)
            | Self::BinaryWrite
            | Self::BytesValidateUtf8
            | Self::BytesContainsInt
            | Self::DequeNew
            | Self::DequeReset
            | Self::DequeCopy
            | Self::DequeExtendSource
            | Self::NamespaceDelete
            | Self::DictDelete
            | Self::ChainMapGetItem
            | Self::ChainMapSetItem
            | Self::ChainMapGet(_)
            | Self::DictPop
            | Self::DictSetDefault
            | Self::DefaultDictMissing
            | Self::DictTryPop(_)
            | Self::DictGatherBytesInt
            | Self::DictGatherStrInt
            | Self::FileReadline
            | Self::GzipOpen
            | Self::BytesIo
            | Self::StringIo(_)
            | Self::ContextVar(_)
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
            | Self::LengthResult
            | Self::ParseArgs { .. }
            | Self::ArgparseText { .. }
            | Self::DequeAppend { .. }
            | Self::DequePop { .. }
            | Self::DequeGet { .. }
            | Self::DequeSet
            | Self::DequeDelete { .. }
            | Self::Pop
            | Self::Print
            | Self::SortRestore
            | Self::StderrLine
            | Self::SysStdout
            | Self::SysOriginalOutput { .. }
            | Self::SysStdoutSet
            | Self::SysStdoutDelete
            | Self::TextOptionalReceiver
            | Self::TextMetadata { .. }
            | Self::SysHasStdout
            | Self::BuiltinsLast
            | Self::BuiltinsLen
            | Self::BuiltinsSetLen
            | Self::BuiltinsHasLast
            | Self::BuiltinsSetLast
            | Self::BuiltinsDeleteLast
            | Self::SysDisplayHook
            | Self::SysStreamWrite { .. }
            | Self::SysStreamFlush { .. }
            | Self::SysGetFrameModuleName
            | Self::SysSetRecursionLimit
            | Self::GeneratorFrameEnter(_)
            | Self::RaiseException
            | Self::SystemExitNew { .. }
            | Self::SystemExitCode
            | Self::SystemExitCodeEqual { .. }
            | Self::SysImplementation
            | Self::ExceptionFieldIdentity { .. }
            | Self::SysThreadInfo
            | Self::SysThreadText { .. }
            | Self::SysStdlibModuleNames
            | Self::SysVersionInfo { .. }
            | Self::SysPlatform
            | Self::SysStreamText { .. }
            | Self::SysEncoding(_)
            | Self::OsName
            | Self::OptionalStrConcat
            | Self::SysOrigArgv
            | Self::DocCaptureStart
            | Self::DocCaptureTake
            | Self::AsciiEscape
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
            | Self::TupleBufferNew
            | Self::TupleBufferPush
            | Self::TupleBufferFinish
            | Self::TupleIndex
            | Self::Islice
            | Self::Cycle => Contract::new(true, PreservesLists),
            Self::BytesIter
            | Self::DefaultListDict
            | Self::DefaultFactorySet { .. }
            | Self::DequeMaxlen
            | Self::DequeIter { .. }
            | Self::OrderedDict
            | Self::CounterNew
            | Self::ChainMapNew
            | Self::ChainMapMaps
            | Self::ChainMapSetMaps
            | Self::CounterElements
            | Self::CounterTotal
            | Self::DictAsBase
            | Self::DictClear
            | Self::ListClear
            | Self::DictIter(_)
            | Self::GlobalSet(_)
            | Self::ExactRecord(_)
            | Self::RecordView
            | Self::ListAllI32
            | Self::ListDistinct
            | Self::DequeClear
            | Self::DequeReverse
            | Self::SetIter
            | Self::SplitLinesIter
            | Self::StringIter
            | Self::StringReverseIter
            | Self::TextClosed => Contract::new(false, Unknown),
            Self::CounterMath { .. }
            | Self::CounterMostCommon
            | Self::SortedRecordRows { .. }
            | Self::SortedStringKeys
            | Self::TupleCacheNew
            | Self::Typing(_)
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
            Self::TupleGet => {
                matches!(args, [tuple, index] if index.ty == Type::Int && types.homogeneous_tuple_element(tuple.ty) == Some(result))
            }
            Self::JoinBufferNew => {
                types.element(result) == Some(Type::Str)
                    && matches!(args, [separator] if separator.ty == Type::Str)
            }
            Self::JoinBufferAppend => {
                result == Type::None
                    && matches!(args, [buffer, piece]
                    if types.element(buffer.ty) == Some(Type::Str) && piece.ty == Type::Str)
            }
            Self::JoinBufferFinish => {
                result == Type::Str
                    && matches!(args, [buffer] if types.element(buffer.ty) == Some(Type::Str))
            }
            Self::ListGetI32 => signature(&[Type::ListInt, Type::Int], Type::Int),
            Self::ListGetBounded { min, max } | Self::ListGetBoundedInBounds { min, max } => {
                signature(&[Type::ListInt, Type::Int], Type::Int)
                    && bounds(*min, *max)
                    && (!matches!(self, Self::ListGetBoundedInBounds { .. })
                        || (matches!(args[0].kind, ExprKind::Local(_)) && index(&args[1])))
            }
            Self::SequenceGetInBounds | Self::SequenceTakeInBounds => {
                matches!(args, [source, cursor]
                if matches!(source.ty, Type::List(_))
                    && types.element(source.ty) == Some(result)
                    && matches!(source.kind, ExprKind::Local(_))
                    && cursor.ty == Type::Int && index(cursor))
            }
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
            Self::FunctionWrappedSet => {
                matches!(result, Type::Callable(_)) && signature(&[result, result], result)
            }
            Self::FunctionWrappedGet => {
                matches!(result, Type::Callable(_)) && signature(&[result], result)
            }
            Self::WrapperAttributes(fields) => {
                args.is_empty() && result == types.tuple(vec![Type::Str; fields.names().len()])
            }
            Self::CallableContext => {
                matches!(args, [callable] if types.cached_function(callable.ty).map(|c| c.frame)
                    .or_else(|| types.partial_function(callable.ty).map(|p| p.frame))
                    .or_else(|| types.dispatcher(callable.ty).map(|d| d.frame)) == Some(result))
            }
            Self::EmptyPartial => {
                matches!(args, [target] if matches!(result, Type::Callable(_)) && target.ty == result)
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
    (name.starts_with("imre_socket_")
        && !matches!(name, "imre_socket_fileno" | "imre_socket_getblocking"))
        || name.starts_with("fn_")
        || matches!(
            name,
            "int_floordiv"
                | "imre_seq_len_fast"
                | "imre_seq_len"
                | "imre_str_list_nonempty"
                | "imre_union_truthy"
                | "imre_seq_reverse"
                | "imre_sort_detach"
                | "imre_list_extend_unhinted"
                | "imre_chr"
                | "imre_annotation_ref"
                | "imre_typing_call_stub"
                | "imre_random_bits"
                | "imre_random_choice_index"
                | "imre_random_progression"
                | "imre_function_get_wrapped"
                | "imre_int_pow"
                | "imre_int_modpow"
                | "imre_int_negative_pow"
                | "imre_patch_parse"
                | "imre_json_index"
                | "imre_json_decoder_decode"
                | "imre_exception_args"
                | "imre_exception_message"
                | "imre_import_error_reject_reset"
                | "imre_exception_repr"
                | "imre_json_iter"
                | "imre_json_object_keys"
                | "imre_json_len"
                | "imre_json_get_receiver"
                | "imre_json_add"
                | "imre_json_empty_object"
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
                | "imre_calendar_datetime"
                | "imre_calendar_date"
                | "imre_calendar_combine"
                | "imre_calendar_delta"
                | "imre_calendar_zone"
                | "imre_calendar_now"
                | "imre_email_parsedate"
                | "imre_email_format_datetime"
                | "imre_email_formatdate"
                | "imre_calendar_shift"
                | "imre_calendar_negate"
                | "imre_calendar_compare"
                | "imre_calendar_date_order"
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
                | "imre_str_percent_tuple"
                | "imre_formatter_format"
                | "imre_formatter_time"
                | "imre_formatter_message"
                | "imre_logrecord_message"
                | "imre_logrecord_repr"
                | "imre_log_dict_config"
                | "imre_log_typed_dict_config"
                | "imre_log_file_config"
                | "imre_log_check_level"
                | "imre_log_lock_acquire"
                | "imre_log_lock_acquire_int"
                | "imre_log_lock_enter"
                | "imre_log_lock_exit"
                | "imre_log_lock_release"
                | "imre_log_invalid_level"
                | "imre_log_manager_new"
                | "imre_log_manager"
                | "imre_log_is_enabled"
                | "imre_stderr_line"
                | "imre_enum_lookup"
                | "imre_enum_lookup_int"
                | "imre_str_unquote"
                | "imre_string_iterable_list"
                | "imre_str_join"
                | "imre_join_buffer_new"
                | "imre_join_buffer_finish"
                | "imre_str_replace"
                | "imre_print_str"
                | "imre_print_bytes"
                | "imre_bytes_str"
                | "imre_bytes_count"
                | "imre_bytes_list"
                | "imre_bytes_contains_int"
                | "imre_byte_output_finish"
                | "imre_bytes_get"
                | "imre_bytes_get_index"
                | "imre_bytes_slice"
                | "imre_optional_str_concat"
                | "imre_str_concat"
                | "imre_bytes_concat"
                | "imre_bytes_repeat"
                | "imre_counter_update"
                | "imre_dict_update"
                | "imre_key_union"
                | "imre_key_intersection"
                | "imre_key_difference"
                | "imre_regex_groupdict"
                | "imre_bytes_decode_splitlines"
                | "imre_str_splitlines"
                | "imre_splitlines_joined_tail"
                | "imre_str_splitlines_options"
                | "imre_str_translate"
                | "imre_mapping_object_get"
                | "imre_exception_field_repr"
                | "imre_system_exit_code_equal"
                | "imre_system_exit_code_text"
                | "imre_exception_field_index"
                | "imre_exception_formatter_result"
                | "imre_exception_field_len"
                | "imre_bytes_decode"
                | "imre_bytes_decode_ascii"
                | "imre_fnmatch_translate"
                | "imre_fnmatch_translate_bytes"
                | "imre_urlsafe_b64encode"
                | "imre_codec_lookup"
                | "imre_str_encode_options"
                | "imre_codec_encode"
                | "imre_codec_decode"
                | "imre_bytes_io_value"
                | "imre_codec_writer_encoding"
                | "imre_codec_writer_write"
                | "imre_codec_writer_control"
                | "imre_hash_new"
                | "imre_sha1_new"
                | "imre_sha256_new"
                | "imre_sha1_hex"
                | "imre_sha256_hex"
                | "imre_hash_copy"
                | "imre_hash_name"
                | "imre_hash_digest"
                | "imre_hash_hexdigest"
                | "imre_hash_algorithms_available"
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
                | "imre_sys_stdout"
                | "imre_sys_stdout_del"
                | "imre_text_optional_receiver"
                | "imre_builtins_last"
                | "imre_builtins_del_last"
                | "imre_sys_displayhook"
                | "imre_sys_filesystem_encoding"
                | "imre_sys_argv"
                | "imre_sys_orig_argv"
                | "imre_sys_getframemodulename"
                | "imre_sys_setrecursionlimit"
                | "imre_path_as_posix"
                | "imre_path_join"
                | "imre_path_join_object"
                | "imre_path_parent"
                | "imre_path_read_bytes"
                | "imre_path_read_text"
                | "imre_path_write_text"
                | "imre_path_read_text_options"
                | "imre_path_write_text_options"
                | "imre_path_open_text_output"
                | "imre_path_write_type_error"
                | "imre_path_write_bytes_union"
                | "imre_path_write_text_union"
                | "imre_path_write_bytes"
                | "imre_path_mkdir"
                | "imre_path_unlink"
                | "imre_path_rmdir"
                | "imre_path_readlink"
                | "imre_path_link"
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
                | "imre_string_io"
                | "imre_string_io_newlines"
                | "imre_string_io_capable"
                | "imre_string_io_readlines"
                | "imre_string_io_value"
                | "imre_string_io_read"
                | "imre_string_io_seek"
                | "imre_string_io_position"
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
                | "imre_int_str"
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
                | "imre_str_expandtabs"
                | "imre_str_lower"
                | "imre_str_upper"
                | "imre_str_capitalize"
                | "imre_module_cache_from_origin"
                | "imre_cache_path_tag"
                | "imre_cache_from_source"
                | "imre_source_from_cache"
                | "imre_str_slice"
                | "imre_str_int_list"
                | "imre_seq_get"
                | "imre_seq_set"
                | "imre_seq_get_index"
                | "imre_seq_set_index"
                | "imre_deque_new"
                | "imre_deque_reset"
                | "imre_deque_append"
                | "imre_deque_copy"
                | "imre_deque_extend_source"
                | "imre_deque_pop"
                | "imre_seq_pop"
                | "imre_seq_repeat"
                | "imre_str_repeat"
                | "imre_seq_slice"
                | "imre_str_list_copy"
                | "imre_record_shallow_copy"
                | "imre_dict_copy"
                | "imre_dict_fromkeys_target"
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
