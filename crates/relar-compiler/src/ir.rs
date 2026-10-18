//! Closed, representation-checked IR. Lowering preserves owned results and may
//! borrow rooted locals for the duration of operator and call evaluation.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Type {
    Int,
    Bool,
    Str,
    ListInt,
    Class(usize),
    List(usize),
    Dict(usize),
    Set(usize),
    Tuple(usize),
    Optional(usize),
    /// An internal mapping cursor, never available in user annotations.
    Iterator,
    /// A sealed builtin exception with an immutable category and string message.
    Exception,
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
            _ => {}
        }
        f.write_str(match self {
            Self::Int => "int",
            Self::Bool => "bool",
            Self::Str => "str",
            Self::ListInt => "list[int]",
            Self::None => "None",
            Self::Iterator => "iterator",
            Self::Exception => "Exception",
            Self::Class(_)
            | Self::List(_)
            | Self::Dict(_)
            | Self::Set(_)
            | Self::Tuple(_)
            | Self::Optional(_) => {
                unreachable!()
            }
        })
    }
}

pub(crate) struct Module {
    pub functions: Vec<Function>,
    pub entry: Function,
}

pub(crate) struct Function {
    pub params: Vec<usize>,
    pub returns: Type,
    pub locals: Vec<Type>,
    pub body: Vec<Statement>,
}

#[derive(Clone)]
pub(crate) struct Expr {
    pub ty: Type,
    pub kind: ExprKind,
}

#[derive(Clone)]
pub(crate) enum ExprKind {
    Int(String),
    Bool(bool),
    Str(String),
    None,
    Local(usize),
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
    Not(Box<Expr>),
    Negate(Box<Expr>),
    Call(usize, Vec<Expr>),
    Construct {
        initializer: usize,
        fields: usize,
        args: Vec<Expr>,
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
    FloorDivide,
    Modulo,
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
    Print,
    Input,
    Int,
    Len,
    ListGet,
    Append,
    Pop,
    IsNone,
    Split,
    DictGet,
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
    While(Expr, Vec<Statement>),
    For {
        target: usize,
        cursor: usize,
        stop: usize,
        step: usize,
        args: Vec<Expr>,
        body: Vec<Statement>,
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
