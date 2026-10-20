//! Closed, representation-checked IR. Lowering preserves owned results and may
//! borrow rooted locals for the duration of operator and call evaluation.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Type {
    Int,
    Float,
    Bool,
    Str,
    Bytes,
    ListInt,
    Class(usize),
    List(usize),
    Dict(usize),
    Set(usize),
    Tuple(usize),
    Optional(usize),
    /// A closed str/bytes union with one owned tagged-pointer word.
    Union(usize),
    /// An internal container or string cursor, never available in user annotations.
    Iterator,
    /// A sealed builtin exception with an immutable category and string message.
    Exception,
    /// A sealed UTF-8 text stream; aliases share its open/closed state.
    TextIO,
    /// A sealed byte stream; aliases share its open/closed state.
    BinaryIO,
    None,
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Self::Class(id) = self {
            return write!(f, "class#{id}");
        }
        match self {
            Self::List(id) => return write!(f, "list#{id}"),
            Self::Dict(id) => return write!(f, "dict#{id}"),
            Self::Set(id) => return write!(f, "set#{id}"),
            Self::Tuple(id) => return write!(f, "tuple#{id}"),
            Self::Optional(id) => return write!(f, "optional#{id}"),
            Self::Union(id) => return write!(f, "union#{id}"),
            _ => {}
        }
        f.write_str(match self {
            Self::Int => "int",
            Self::Float => "float",
            Self::Bool => "bool",
            Self::Str => "str",
            Self::Bytes => "bytes",
            Self::ListInt => "list[int]",
            Self::None => "None",
            Self::Iterator => "iterator",
            Self::Exception => "Exception",
            Self::TextIO => "typing.TextIO",
            Self::BinaryIO => "typing.BinaryIO",
            Self::Class(_)
            | Self::List(_)
            | Self::Dict(_)
            | Self::Set(_)
            | Self::Tuple(_)
            | Self::Optional(_)
            | Self::Union(_) => {
                unreachable!()
            }
        })
    }
}

pub(crate) struct Module {
    /// Exact storage shapes survive checking so lowering need not erase them to
    /// generic tagged slots. Class identities index `class_fields`; container
    /// identities refer to this compilation's interned type universe.
    pub types: crate::types::Types,
    pub class_fields: Vec<Vec<Type>>,
    /// Fixed module bindings own their exact storage until process cleanup.
    pub globals: Vec<Type>,
    pub functions: Vec<Function>,
    pub entry: Function,
}

impl Module {
    /// Process argument state is required only by an actual argv expression.
    /// Merely importing sys emits no startup allocation or runtime query.
    pub(crate) fn uses_sys_argv(&self) -> bool {
        let mut used = false;
        for function in self.functions.iter().chain([&self.entry]) {
            walk_expressions(&function.body, &mut |value| {
                used |= matches!(value.kind, ExprKind::Builtin(Builtin::SysArgv, _));
            });
        }
        used
    }
}

pub(crate) struct Function {
    pub params: Vec<usize>,
    pub returns: Type,
    pub locals: Vec<Type>,
    pub body: Vec<Statement>,
}

impl Function {
    /// These slots can change while an earlier operand is still being used.
    /// Comprehension targets stay private; only explicit binding expressions
    /// write into the enclosing source scope.
    pub(crate) fn expression_rebound(&self) -> Vec<bool> {
        let mut rebound = vec![false; self.locals.len()];
        walk_expressions(&self.body, &mut |value| {
            if let ExprKind::Bind(slot, _) = value.kind {
                rebound[slot] = true;
            }
        });
        rebound
    }
}

#[derive(Clone)]
pub(crate) struct Expr {
    pub ty: Type,
    pub kind: ExprKind,
}

/// Values are evaluated and rooted in source order. `order` is a permutation of
/// their indices that supplies the native call's parameter order without
/// reevaluation. Methods include their receiver first; constructors omit the
/// implicit receiver, which is allocated only after these values are evaluated.
#[derive(Clone)]
pub(crate) struct CallArgs {
    pub values: Vec<Expr>,
    pub order: Vec<usize>,
}

impl CallArgs {
    /// Uses the existing positional order for calls that need no argument binding.
    pub(crate) fn positional(values: Vec<Expr>) -> Self {
        Self {
            order: (0..values.len()).collect(),
            values,
        }
    }
}

#[derive(Clone)]
pub(crate) enum ExprKind {
    Int(String),
    Float(f64),
    Bool(bool),
    Str(String),
    Bytes(Vec<u8>),
    None,
    Local(usize),
    /// Stores the value, promoting its storage type if needed, and yields the
    /// original value with an independent owned reference.
    Bind(usize, Box<Expr>),
    /// An eager comprehension with writes to its enclosing source scope.
    /// Private slots are cleared on success and on every escaping error.
    Scope {
        body: Vec<Statement>,
        value: Box<Expr>,
        clear: Vec<usize>,
    },
    List(Vec<Expr>),
    Dict {
        key: Type,
        value: Type,
        entries: Vec<(Expr, Expr)>,
    },
    Set {
        element: Type,
        values: Vec<Expr>,
    },
    Tuple(Vec<Expr>),
    Present(Box<Expr>),
    Absent(Box<Expr>),
    Unwrap(Box<Expr>),
    Binary(Binary, Box<Expr>, Box<Expr>),
    Compare(Vec<Expr>, Vec<Compare>),
    Boolean(bool, Vec<Expr>),
    /// Returns the short-circuit operand, narrowed by its selected truth value,
    /// or the RHS. The operands are evaluated once and retain their identities.
    BooleanValue(bool, Box<Expr>, Box<Expr>),
    /// Only the selected arm runs; both arms have the expression's storage type.
    Conditional(Box<Expr>, Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Negate(Box<Expr>),
    Call(usize, CallArgs),
    Construct {
        initializer: usize,
        args: CallArgs,
    },
    Exception {
        kind: u32,
        /// None distinguishes a zero-argument exception from an empty string.
        message: Option<Box<Expr>>,
    },
    Field(Box<Expr>, usize),
    Builtin(Builtin, Vec<Expr>),
}

#[derive(Clone, Copy)]
pub(crate) enum Binary {
    Add,
    Subtract,
    Multiply,
    Divide,
    FloorDivide,
    Modulo,
    BitAnd,
    BitOr,
    BitXor,
    LeftShift,
    RightShift,
    Repeat,
    Concat,
}

#[derive(Clone, Copy)]
pub(crate) enum Compare {
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
}

#[derive(Clone)]
pub(crate) enum Builtin {
    /// Reads an initialized module binding with an independent owned reference.
    GlobalGet(usize),
    /// Publishes one fixed module binding after its initializer succeeds.
    GlobalSet(usize),
    Print,
    Input,
    Int,
    Float,
    Bool,
    Str,
    Len,
    ListGet,
    /// Reduces an exact integer list after evaluating its source and start.
    /// Integer addition cannot invoke callbacks or raise a recoverable error.
    ListSum,
    /// Sums a private integer slice at the slice's original evaluation point.
    /// Nullable bounds and zero-step errors match ordinary slicing.
    ListSliceSum,
    /// Internal loop guard over [list, start, stop, step]. Scanning is limited
    /// to four elements per planned iteration; true proves every value fits i32.
    ListAllI32,
    /// Best-effort bounded capacity hint for one empty loop output list.
    ListReserveRange,
    /// The guarded loop proves all elements fit i32 and cannot be replaced.
    ListGetI32,
    /// Every call site and intervening operation preserve these exact element
    /// bounds. The access still checks its index and returns a tagged-small Int.
    ListGetBounded {
        min: i64,
        max: i64,
    },
    /// A canonical reduction loop proves its direct source/cursor access is
    /// in bounds before advancing. The rooted list cannot change during the
    /// loop; the borrowed result also has these exact tagged-small bounds.
    ListGetBoundedInBounds {
        min: i64,
        max: i64,
    },
    /// A checked reduction version proves this canonical integer identity.
    IntBounded {
        min: i64,
        max: i64,
    },
    /// Internal identity guard between a read source and a mutated list.
    ListDistinct,
    StringGet,
    StringIter,
    /// Linear Unicode iteration from the final code point toward the first.
    StringReverseIter,
    /// Private, initially zeroed tuple storage for iterator result caching.
    TupleCacheNew,
    /// Reuses an exclusively owned iterator cache or allocates a separate result.
    TupleCacheResult,
    /// Eagerly converts immutable string code points into an integer list,
    /// preserving `list(map(int, text))` errors without character objects.
    StringIntList,
    Bytes,
    BytesGet,
    BytesIter,
    Encode,
    Decode,
    /// Validates complete UTF-8 at the decode evaluation point and returns a
    /// retained bytes identity for an internal allocation-free character gather.
    BytesValidateUtf8,
    /// Receiver followed by three nullable integer words for start, stop, step.
    Slice,
    Append,
    Pop,
    IsNone,
    /// Promotes an exact str/bytes owner without allocating or changing identity.
    UnionInject,
    /// Projects a member proved by local control flow, acquiring its own owner.
    UnionProject,
    IsInstance(Vec<InstanceType>),
    Split,
    SplitLines,
    RStrip,
    SysArgv,
    /// Zero reads, one truncates/writes; the frontend accepts literal modes only.
    TextOpen(u32),
    BinaryOpen(u32),
    BinaryRead,
    BinaryWrite,
    TextEnter,
    TextRead,
    TextWrite,
    TextFlush,
    TextClose,
    TextClosed,
    DictGet,
    /// Maps each Unicode scalar through an exact string-to-integer dictionary.
    /// The text is evaluated first; the mapping remains mutable between calls.
    DictGatherStrInt,
    /// Reads immutable bytes already proved valid by BytesValidateUtf8.
    DictGatherBytesInt,
    DictGetDefault,
    /// The stored value type determines optional promotion on successful lookup.
    /// A third argument, when present, already has the flattened result type.
    DictTryGet(Type),
    /// Arguments are key then dictionary, matching membership evaluation order.
    DictContains,
    DictDelete,
    /// False visits keys; true visits key/value pairs without an intermediate tuple.
    DictIter(bool),
    /// Arguments are the element then set, matching membership evaluation order.
    SetContains,
    SetAdd,
    SetDiscard,
    SetIter,
    ExceptionMessage,
    /// Attaches an explicit cause to a fresh, unescaped builtin exception.
    ExceptionWithCause,
    ExceptionCause,
    ExceptionSuppressContext,
}

/// Sealed Python runtime categories; parameterized container annotations are
/// never runtime classes, and exception categories use their existing hierarchy.
#[derive(Clone, Copy)]
pub(crate) enum InstanceType {
    Exact(Type),
    List,
    Dict,
    Set,
    Tuple,
    Exception(u32),
}

impl InstanceType {
    pub(crate) fn accepts(self, ty: Type) -> bool {
        match self {
            Self::Exact(Type::Int) => matches!(ty, Type::Int | Type::Bool),
            Self::Exact(expected) => expected == ty,
            Self::List => matches!(ty, Type::ListInt | Type::List(_)),
            Self::Dict => matches!(ty, Type::Dict(_)),
            Self::Set => matches!(ty, Type::Set(_)),
            Self::Tuple => matches!(ty, Type::Tuple(_)),
            Self::Exception(_) => ty == Type::Exception,
        }
    }
}

#[derive(Clone)]
pub(crate) struct ExceptHandler {
    /// Empty for a bare handler; otherwise fixed builtin exception categories.
    pub kinds: Vec<u32>,
    pub name: Option<usize>,
    pub body: Vec<Statement>,
}

#[derive(Clone)]
pub(crate) enum Statement {
    Assign(usize, Expr),
    /// Transfers an owned local to another local, resetting its source.
    Move(usize, usize),
    /// Releases an owned temporary and restores its cleanup-safe initial value.
    Clear(usize),
    /// Compiler-owned roots are released on normal and abrupt scope exits.
    Scope {
        body: Vec<Statement>,
        clear: Vec<usize>,
    },
    SetItem(Expr, Expr, Expr),
    AugmentItem(Expr, Expr, Binary, Expr),
    SetField(Expr, usize, Expr),
    AugmentField(Expr, usize, Binary, Expr),
    Expr(Expr),
    If(Expr, Vec<Statement>, Vec<Statement>),
    /// The final suite runs only on condition failure or iterator exhaustion.
    While(Expr, Vec<Statement>, Vec<Statement>),
    For {
        target: usize,
        cursor: usize,
        stop: usize,
        step: usize,
        args: Vec<Expr>,
        body: Vec<Statement>,
        orelse: Vec<Statement>,
    },
    /// Transfers iterator results into owned output locals only on success.
    /// `ready` is always written; exhausted cursors preserve both output locals.
    IterNext {
        iterator: usize,
        key: usize,
        value: Option<usize>,
        ready: usize,
    },
    Return(Expr),
    Break,
    Continue,
    /// A compiler-owned iterator reached its end; unlike break, this runs else.
    Exhaust,
    Assert(Expr),
    /// Handlers protect only `body`; `finalbody` runs for every exit reason.
    Try {
        body: Vec<Statement>,
        handlers: Vec<ExceptHandler>,
        orelse: Vec<Statement>,
        finalbody: Vec<Statement>,
    },
    /// None re-raises the dynamically active exception, including across calls.
    Raise(Option<Expr>),
}

/// Visits expression trees, including statements embedded in eager expressions.
/// Analyses must include every checked branch, even when it never executes.
pub(crate) fn walk_expressions(body: &[Statement], visit: &mut impl FnMut(&Expr)) {
    fn expression(value: &Expr, visit: &mut impl FnMut(&Expr)) {
        visit(value);
        match &value.kind {
            ExprKind::List(values)
            | ExprKind::Tuple(values)
            | ExprKind::Set { values, .. }
            | ExprKind::Compare(values, _)
            | ExprKind::Boolean(_, values)
            | ExprKind::Builtin(_, values) => {
                for value in values {
                    expression(value, visit);
                }
            }
            ExprKind::Call(_, args) | ExprKind::Construct { args, .. } => {
                for value in &args.values {
                    expression(value, visit);
                }
            }
            ExprKind::Dict { entries, .. } => {
                for (key, value) in entries {
                    expression(key, visit);
                    expression(value, visit);
                }
            }
            ExprKind::Bind(_, value)
            | ExprKind::Present(value)
            | ExprKind::Absent(value)
            | ExprKind::Unwrap(value)
            | ExprKind::Not(value)
            | ExprKind::Negate(value)
            | ExprKind::Field(value, _) => expression(value, visit),
            ExprKind::Scope { body, value, .. } => {
                walk_expressions(body, visit);
                expression(value, visit);
            }
            ExprKind::Binary(_, left, right) | ExprKind::BooleanValue(_, left, right) => {
                expression(left, visit);
                expression(right, visit);
            }
            ExprKind::Conditional(test, yes, no) => {
                expression(test, visit);
                expression(yes, visit);
                expression(no, visit);
            }
            ExprKind::Exception { message, .. } => {
                if let Some(message) = message {
                    expression(message, visit);
                }
            }
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Str(_)
            | ExprKind::Bytes(_)
            | ExprKind::None
            | ExprKind::Local(_) => {}
        }
    }
    for statement in body {
        match statement {
            Statement::Assign(_, value)
            | Statement::Expr(value)
            | Statement::Return(value)
            | Statement::Assert(value) => expression(value, visit),
            Statement::SetItem(receiver, index, value)
            | Statement::AugmentItem(receiver, index, _, value) => {
                expression(receiver, visit);
                expression(index, visit);
                expression(value, visit);
            }
            Statement::SetField(receiver, _, value)
            | Statement::AugmentField(receiver, _, _, value) => {
                expression(receiver, visit);
                expression(value, visit);
            }
            Statement::Scope { body, .. } => walk_expressions(body, visit),
            Statement::If(test, yes, no) => {
                expression(test, visit);
                walk_expressions(yes, visit);
                walk_expressions(no, visit);
            }
            Statement::While(test, body, orelse) => {
                expression(test, visit);
                walk_expressions(body, visit);
                walk_expressions(orelse, visit);
            }
            Statement::For {
                args, body, orelse, ..
            } => {
                for arg in args {
                    expression(arg, visit);
                }
                walk_expressions(body, visit);
                walk_expressions(orelse, visit);
            }
            Statement::Try {
                body,
                handlers,
                orelse,
                finalbody,
            } => {
                walk_expressions(body, visit);
                for handler in handlers {
                    walk_expressions(&handler.body, visit);
                }
                walk_expressions(orelse, visit);
                walk_expressions(finalbody, visit);
            }
            Statement::Raise(value) => {
                if let Some(value) = value {
                    expression(value, visit);
                }
            }
            Statement::Move(..)
            | Statement::Clear(_)
            | Statement::IterNext { .. }
            | Statement::Break
            | Statement::Continue
            | Statement::Exhaust => {}
        }
    }
}
