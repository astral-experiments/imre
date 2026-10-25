//! Lower pure integer terms into the shared, owned locals of one zip row.

use std::collections::HashMap;
use std::mem::{Discriminant, discriminant};

use crate::ir::{Binary, CallArgs, Expr, ExprKind, Function, Statement, Type};

#[derive(Eq, Hash, PartialEq)]
enum Operand {
    Slot(usize),
    Constant(String),
}

impl Operand {
    fn of(value: &Expr) -> Option<Self> {
        match &value.kind {
            ExprKind::Local(slot) => Some(Self::Slot(*slot)),
            ExprKind::Int(n) => Some(Self::Constant(n.clone())),
            _ => None,
        }
    }
}

#[derive(Eq, Hash, PartialEq)]
enum Operation {
    Field(usize, usize),
    Negate(Operand),
    Binary(Discriminant<Binary>, Operand, Operand),
}

impl Operation {
    fn of(kind: &ExprKind) -> Option<Self> {
        match kind {
            ExprKind::Field(owner, field) => match owner.kind {
                ExprKind::Local(slot) => Some(Self::Field(slot, *field)),
                _ => None,
            },
            ExprKind::Negate(value) => Some(Self::Negate(Operand::of(value)?)),
            ExprKind::Binary(op, a, b) => Some(Self::Binary(
                discriminant(op),
                Operand::of(a)?,
                Operand::of(b)?,
            )),
            _ => None,
        }
    }
}

fn local(slot: usize, ty: Type) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Local(slot),
    }
}

pub(super) struct Row {
    items: [usize; 2],
    shared: HashMap<Operation, usize>,
}

impl Row {
    pub(super) fn new(items: [usize; 2]) -> Self {
        Self {
            items,
            shared: HashMap::new(),
        }
    }

    /// Remap this generator's pair to the first generator's row. A rejected
    /// term leaves both the row and the caller's output unchanged.
    pub(super) fn project(
        &mut self,
        term: &Expr,
        slots: [usize; 2],
        functions: &[Function],
        locals: &mut Vec<Type>,
        body: &mut Vec<Statement>,
    ) -> Option<Expr> {
        if term.ty != Type::Int || slots[0] == slots[1] {
            return None;
        }
        let bindings = [
            (slots[0], local(self.items[0], *locals.get(self.items[0])?)),
            (slots[1], local(self.items[1], *locals.get(self.items[1])?)),
        ];
        let first_local = locals.len();
        let first_statement = body.len();
        let result = Projection {
            row: self,
            functions,
            locals,
            body,
            remaining: 64,
        }
        .value(term, &bindings, true);
        if result.is_none() {
            self.shared.retain(|_, slot| *slot < first_local);
            locals.truncate(first_local);
            body.truncate(first_statement);
        }
        result
    }
}

struct Projection<'a> {
    row: &'a mut Row,
    functions: &'a [Function],
    locals: &'a mut Vec<Type>,
    body: &'a mut Vec<Statement>,
    // Bounds expansion, including calls to recursive or multiply used helpers.
    remaining: usize,
}

impl Projection<'_> {
    fn store(&mut self, value: Expr) -> Expr {
        let slot = self.locals.len();
        let ty = value.ty;
        self.locals.push(ty);
        self.body.push(Statement::Assign(slot, value));
        local(slot, ty)
    }

    fn value(&mut self, old: &Expr, bindings: &[(usize, Expr)], eager: bool) -> Option<Expr> {
        self.remaining = self.remaining.checked_sub(1)?;
        let ty = old.ty;
        let kind = match &old.kind {
            ExprKind::Int(_) if ty == Type::Int => return Some(old.clone()),
            ExprKind::Bool(_) if ty == Type::Bool => return Some(old.clone()),
            ExprKind::Local(slot) => {
                let (_, value) = bindings.iter().rev().find(|(id, _)| id == slot)?;
                return (value.ty == ty).then(|| value.clone());
            }
            ExprKind::Field(owner, field)
                if ty == Type::Int && matches!(owner.ty, Type::Class(_) | Type::Tuple(_)) =>
            {
                ExprKind::Field(Box::new(self.value(owner, bindings, eager)?), *field)
            }
            ExprKind::Negate(value) if ty == Type::Int && value.ty == Type::Int => {
                ExprKind::Negate(Box::new(self.value(value, bindings, eager)?))
            }
            ExprKind::Binary(op @ (Binary::Add | Binary::Subtract | Binary::Multiply), a, b)
                if ty == Type::Int && a.ty == Type::Int && b.ty == Type::Int =>
            {
                ExprKind::Binary(
                    *op,
                    Box::new(self.value(a, bindings, eager)?),
                    Box::new(self.value(b, bindings, eager)?),
                )
            }
            ExprKind::Compare(values, ops) if ty == Type::Bool => {
                let [a, b] = values.as_slice() else {
                    return None;
                };
                if ops.len() != 1 || a.ty != Type::Int || b.ty != Type::Int {
                    return None;
                }
                ExprKind::Compare(
                    vec![
                        self.value(a, bindings, eager)?,
                        self.value(b, bindings, eager)?,
                    ],
                    ops.clone(),
                )
            }
            ExprKind::Conditional(test, yes, no)
                if ty == Type::Int && test.ty == Type::Bool && yes.ty == ty && no.ty == ty =>
            {
                // A branch can reuse a dominating value. New computations in
                // its arm stay lazy, including potentially large integer products.
                ExprKind::Conditional(
                    Box::new(self.value(test, bindings, eager)?),
                    Box::new(self.value(yes, bindings, false)?),
                    Box::new(self.value(no, bindings, false)?),
                )
            }
            ExprKind::Call(id, args) if eager && ty == Type::Int => {
                return self.call(*id, args, bindings);
            }
            _ => return None,
        };
        let operation = Operation::of(&kind);
        let value = Expr { ty, kind };
        if let Some(operation) = operation {
            if let Some(&slot) = self.row.shared.get(&operation) {
                return Some(local(slot, ty));
            }
            if eager {
                self.row.shared.insert(operation, self.locals.len());
                return Some(self.store(value));
            }
        }
        Some(value)
    }

    /// Every argument and assignment is checked, including unused ones. Only
    /// private scalar bindings and a single return can survive substitution.
    fn call(&mut self, id: usize, args: &CallArgs, caller: &[(usize, Expr)]) -> Option<Expr> {
        let function = self.functions.get(id)?;
        let (Statement::Return(result), assignments) = function.body.split_last()? else {
            return None;
        };
        if function.returns != Type::Int
            || result.ty != Type::Int
            || function.params.len() != args.order.len()
            || args.values.len() != args.order.len()
            || args.values.len() + assignments.len() > self.remaining
        {
            return None;
        }
        let mut values = Vec::new();
        for arg in &args.values {
            if arg.ty != Type::Int {
                return None;
            }
            values.push(self.binding(arg, caller)?);
        }
        let mut bindings = Vec::new();
        for (&slot, &index) in function.params.iter().zip(&args.order) {
            if function.locals.get(slot) != Some(&Type::Int) {
                return None;
            }
            bindings.push((slot, values.get(index)?.clone()));
        }
        for statement in assignments {
            let Statement::Assign(slot, rhs) = statement else {
                return None;
            };
            if !matches!(rhs.ty, Type::Int | Type::Bool)
                || function.locals.get(*slot) != Some(&rhs.ty)
            {
                return None;
            }
            let value = self.binding(rhs, &bindings)?;
            bindings.push((*slot, value));
        }
        self.value(result, &bindings, true)
    }

    fn binding(&mut self, value: &Expr, bindings: &[(usize, Expr)]) -> Option<Expr> {
        let value = self.value(value, bindings, true)?;
        Some(match value.kind {
            ExprKind::Local(_) | ExprKind::Int(_) | ExprKind::Bool(_) => value,
            _ => self.store(value),
        })
    }
}
