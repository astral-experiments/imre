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
    Tuple(usize),
    Optional(usize),
    None,
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Self::Class(id) = self {
            return write!(f, "class#{id}");
        }
        match self {
            Self::List(id) => return write!(f, "list#{id}"),
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
            Self::Class(_) | Self::List(_) | Self::Tuple(_) | Self::Optional(_) => unreachable!(),
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
}

pub(crate) enum Statement {
    Assign(usize, Expr),
    /// Transfers an owned local to another local, resetting its source.
    Move(usize, usize),
    /// Releases an owned temporary and restores its cleanup-safe initial value.
    Clear(usize),
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
    Return(Expr),
    Break,
    Continue,
    Assert(Expr),
}
