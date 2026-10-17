//! Closed, representation-checked IR. Lowering preserves owned results and may
//! borrow rooted locals for the duration of operator and call evaluation.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Type {
    Int,
    Bool,
    Str,
    ListInt,
    None,
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Int => "int",
            Self::Bool => "bool",
            Self::Str => "str",
            Self::ListInt => "list[int]",
            Self::None => "None",
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

pub(crate) struct Expr {
    pub ty: Type,
    pub kind: ExprKind,
}

pub(crate) enum ExprKind {
    Int(String),
    Bool(bool),
    Str(String),
    None,
    Local(usize),
    List(Vec<Expr>),
    Binary(Binary, Box<Expr>, Box<Expr>),
    Compare(Vec<Expr>, Vec<Compare>),
    Boolean(bool, Vec<Expr>),
    Not(Box<Expr>),
    Negate(Box<Expr>),
    Call(usize, Vec<Expr>),
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

pub(crate) enum Builtin {
    Print,
    Input,
    Int,
    Len,
    ListGet,
    Append,
    Pop,
}

pub(crate) enum Statement {
    Assign(usize, Expr),
    SetItem(Expr, Expr, Expr),
    AugmentItem(Expr, Expr, Binary, Expr),
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
