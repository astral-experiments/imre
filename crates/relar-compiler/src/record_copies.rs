//! Forward the fields of private frozen-record copies into a field-only reader.
//! The original function and record ABI remain available wherever identity escapes.

use crate::ir::{self, CallArgs, Expr, ExprKind, Function, Module, Statement, Type};
use crate::{ir::children_mut, reductions::count_writes};

type Overrides = Vec<Option<Expr>>;

fn local(slot: usize, ty: Type) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Local(slot),
    }
}

/// A pure constructor forwards each original field or supplies an immediate
/// override. The same summary handles ordinary constructor calls and the private
/// argument scope emitted for `**asdict(self)`.
fn copy(function: &Function, module: &Module) -> Option<Overrides> {
    let &[receiver] = function.params.as_slice() else {
        return None;
    };
    let Type::Class(class) = function.returns else {
        return None;
    };
    // Keeping the original record rooted through a reader must not extend a
    // file handle's lifetime (including handles reachable through containers).
    if !module.class_fields[class].iter().all(|&ty| {
        module.types.asdict_scalar(ty)
            || matches!(
                module.types.payload(ty).unwrap_or(ty),
                Type::Enum(_) | Type::StrEnum(_)
            )
    }) {
        return None;
    }
    let [Statement::Return(value)] = function.body.as_slice() else {
        return None;
    };
    if function.locals[receiver] != function.returns || value.ty != function.returns {
        return None;
    }
    let (value, root, updates) = if let ExprKind::Scope { body, value, clear } = &value.kind {
        let (Statement::Assign(root, input), updates) = body.split_first()? else {
            return None;
        };
        if slot(input) != Some(receiver) || clear.last() != Some(root) || body.len() != clear.len()
        {
            return None;
        }
        for (statement, cleanup) in updates.iter().zip(clear.iter().rev().skip(1)) {
            let Statement::Assign(id, value) = statement else {
                return None;
            };
            if id != cleanup || *id == *root || *id == receiver || !constant(value) {
                return None;
            }
        }
        (value.as_ref(), *root, updates)
    } else {
        (value, receiver, &[][..])
    };
    let ExprKind::Construct { initializer, args } = &value.kind else {
        return None;
    };
    if Some(*initializer) != module.class_metadata[class].frozen_initializer {
        return None;
    }
    let init = &module.functions[*initializer];
    let mut fields = Vec::new();
    for statement in &init.body {
        if matches!(
            statement,
            Statement::Return(Expr {
                kind: ExprKind::None,
                ..
            })
        ) {
            continue;
        }
        let Statement::SetField(base, field, parameter) = statement else {
            return None;
        };
        if slot(base) != Some(init.params[0]) || *field != fields.len() {
            return None;
        }
        let param = slot(parameter)?;
        let pos = init.params[1..].iter().position(|p| *p == param)?;
        let arg = &args.values[args.order[pos]];
        if arg.ty != module.class_fields[class][*field] {
            return None;
        }
        fields.push(match &arg.kind {
            ExprKind::Field(source, i) if slot(source) == Some(root) && i == field => None,
            _ if constant(arg) => Some(arg.clone()),
            ExprKind::Local(id) => Some(updates.iter().rev().find_map(|stmt| match stmt {
                Statement::Assign(s, value) if s == id => Some(value.clone()),
                _ => None,
            })?),
            _ => return None,
        });
    }
    (fields.len() == module.class_fields[class].len()).then_some(fields)
}

fn constant(value: &Expr) -> bool {
    match &value.kind {
        ExprKind::Bool(_) | ExprKind::None => true,
        ExprKind::Present(inner) | ExprKind::Absent(inner) => constant(inner),
        _ => false,
    }
}

struct Reader {
    /// Fields this reader observes which a summarized copy can change.
    fields: Vec<usize>,
    variant: Option<usize>,
}

impl Reader {
    fn new(function: &Function, changed: &[bool]) -> Option<Self> {
        let &[receiver] = function.params.as_slice() else {
            return None;
        };
        let mut writes = vec![0; function.locals.len()];
        count_writes(&function.body, &mut writes);
        if writes[receiver] != 0 {
            return None;
        }
        let mut reads = 0;
        let mut projections = 0;
        let mut fields = Vec::new();
        ir::walk_expressions(&function.body, &mut |value| match &value.kind {
            ExprKind::Local(slot) if *slot == receiver => reads += 1,
            ExprKind::Field(base, field) if slot(base) == Some(receiver) => {
                projections += 1;
                if changed[*field] && !fields.contains(field) {
                    fields.push(*field);
                }
            }
            _ => {}
        });
        (reads != 0 && reads == projections).then_some(Self {
            fields,
            variant: None,
        })
    }

    /// Unchanged fields stay borrowed from the rooted base. Only overwritten
    /// fields cross the call boundary as additional ordinary scalar arguments.
    fn specialize(&self, original: &Function, types: &[Type]) -> Function {
        let mut function = original.clone();
        let start = function.locals.len();
        for &field in &self.fields {
            function.params.push(function.locals.len());
            function.locals.push(types[field]);
        }
        let receiver = original.params[0];
        ir::walk_expressions_mut(&mut function.body, &mut |value| {
            if let ExprKind::Field(base, field) = &value.kind
                && slot(base) == Some(receiver)
                && let Some(i) = self.fields.iter().position(|f| f == field)
            {
                value.kind = ExprKind::Local(start + i);
            }
        });
        function
    }
}

fn slot(value: &Expr) -> Option<usize> {
    if let ExprKind::Local(slot) = value.kind {
        Some(slot)
    } else {
        None
    }
}

struct Copies<'a> {
    methods: Vec<Option<Overrides>>,
    readers: Vec<Option<Reader>>,
    original_count: usize,
    origins: Vec<usize>,
    class_fields: &'a [Vec<Type>],
}

impl Copies<'_> {
    fn unary<'a>(&self, value: &'a Expr) -> Option<(usize, &'a Expr)> {
        let ExprKind::Call(id, args) = &value.kind else {
            return None;
        };
        let [base] = args.values.as_slice() else {
            return None;
        };
        Some((*id, base))
    }

    fn source(&self, value: &Expr) -> Option<(usize, &Overrides)> {
        let (id, base) = self.unary(value)?;
        let ExprKind::Local(root) = base.kind else {
            return None;
        };
        Some((root, self.methods.get(id)?.as_ref()?))
    }

    /// The caller has already rooted the base. Recursively overlay any pure
    /// copies on its fields, stopping at a whole-value observation or unknown call.
    fn fields(&self, value: &Expr, slot: usize, base: &[Expr]) -> Option<Vec<Expr>> {
        if matches!(value.kind, ExprKind::Local(s) if s == slot) {
            return Some(base.to_owned());
        }
        let (id, source) = self.unary(value)?;
        let overrides = self.methods.get(id)?.as_ref()?;
        let mut fields = self.fields(source, slot, base)?;
        for (field, replacement) in fields.iter_mut().zip(overrides) {
            if let Some(value) = replacement {
                *field = value.clone();
            }
        }
        Some(fields)
    }

    fn consume(&mut self, expr: &mut Expr, slot: usize, base: &[Expr]) -> bool {
        if let ExprKind::Field(source, field) = &expr.kind
            && let Some(fields) = self.fields(source, slot, base)
        {
            *expr = fields[*field].clone();
            return true;
        }
        if let Some((id, source)) = self.unary(expr)
            && self.readers.get(id).is_some_and(Option::is_some)
            && let Some(fields) = self.fields(source, slot, base)
        {
            let reader = self.readers[id].as_mut().unwrap();
            let mut args = vec![local(slot, source.ty)];
            let target = if reader.fields.is_empty() {
                id
            } else {
                args.extend(reader.fields.iter().map(|&f| fields[f].clone()));
                *reader.variant.get_or_insert_with(|| {
                    self.origins.push(id);
                    self.original_count + self.origins.len() - 1
                })
            };
            expr.kind = ExprKind::Call(target, CallArgs::positional(args));
            return true;
        }
        if matches!(expr.kind, ExprKind::Scope { .. }) {
            return false;
        }
        let mut changed = false;
        children_mut(expr, |child| changed |= self.consume(child, slot, base));
        changed
    }

    /// A direct chain needs no local assignment or ownership transfer. Each
    /// operand up to the rooted source is already proved to be a pure copy.
    fn direct(&mut self, expr: &mut Expr) {
        let source = match &expr.kind {
            ExprKind::Field(source, _) => source.as_ref(),
            ExprKind::Call(id, args)
                if self.readers.get(*id).is_some_and(Option::is_some) && args.values.len() == 1 =>
            {
                &args.values[0]
            }
            _ => return,
        };
        if !matches!(source.kind, ExprKind::Call(..)) {
            return;
        }
        let mut root = source;
        while let Some((id, inner)) = self.unary(root) {
            if !self.methods.get(id).is_some_and(Option::is_some) {
                return;
            }
            root = inner;
        }
        let (Some(slot), Type::Class(class)) = (slot(root), root.ty) else {
            return;
        };
        let fields = self.class_fields[class]
            .iter()
            .enumerate()
            .map(|(i, &ty)| Expr {
                ty,
                kind: ExprKind::Field(Box::new(root.clone()), i),
            })
            .collect::<Vec<_>>();
        self.consume(expr, slot, &fields);
    }

    fn pair(
        &mut self,
        assignment: &mut Statement,
        next: &mut Statement,
        private: &[bool],
        locals: &mut Vec<Type>,
    ) {
        let Statement::Assign(slot, value) = assignment else {
            return;
        };
        let slot = *slot;
        let Type::Class(class) = value.ty else { return };
        if !private[slot] {
            return;
        }
        let (root, yes, no, test) = match &value.kind {
            ExprKind::Conditional(test, yes, no) => {
                let (Some((root, yes)), Some((other, no))) = (self.source(yes), self.source(no))
                else {
                    return;
                };
                if root != other {
                    return;
                }
                (root, yes, Some(no), Some(test.as_ref().clone()))
            }
            _ => {
                let Some((root, changes)) = self.source(value) else {
                    return;
                };
                (root, changes, None, None)
            }
        };
        let guard = locals.len();
        let fields = self.class_fields[class]
            .iter()
            .enumerate()
            .map(|(i, &ty)| {
                let original = || Expr {
                    ty,
                    kind: ExprKind::Field(Box::new(local(slot, value.ty)), i),
                };
                let then_value = || yes[i].clone().unwrap_or_else(original);
                if let Some(no) = no
                    && (yes[i].is_some() || no[i].is_some())
                {
                    Expr {
                        ty,
                        kind: ExprKind::Conditional(
                            Box::new(local(guard, Type::Bool)),
                            Box::new(then_value()),
                            Box::new(no[i].clone().unwrap_or_else(original)),
                        ),
                    }
                } else {
                    then_value()
                }
            })
            .collect::<Vec<_>>();
        let use_site = match next {
            Statement::Assign(_, value)
            | Statement::Return(value)
            | Statement::Expr(value)
            | Statement::Assert(value)
            | Statement::If(value, _, _) => value,
            _ => return,
        };
        if !self.consume(use_site, slot, &fields) {
            return;
        }
        let ty = value.ty;
        *value = local(root, ty);
        if let Some(test) = test {
            // Evaluate truthiness exactly once, before snapshotting the chosen
            // input. It may rebind the source, raise, or have observable effects.
            locals.push(Type::Bool);
            *value = Expr {
                ty,
                kind: ExprKind::Scope {
                    body: vec![Statement::Assign(guard, test)],
                    value: Box::new(local(root, ty)),
                    clear: Vec::new(),
                },
            };
        }
    }

    fn block(&mut self, body: &mut [Statement], private: &[bool], locals: &mut Vec<Type>) {
        for i in 0..body.len() {
            if let [current, next, ..] = &mut body[i..] {
                self.pair(current, next, private, locals);
            }
            ir::statement_children_mut(&mut body[i], |child| {
                if let ir::ChildMut::Suite(body) = child {
                    self.block(body, private, locals);
                }
            });
        }
    }
}

/// Return each newly generated reader's original function for code-unit ownership.
pub(crate) fn scalarize(module: &mut Module) -> Vec<usize> {
    let methods: Vec<_> = module.functions.iter().map(|f| copy(f, module)).collect();
    if methods.iter().all(Option::is_none) {
        return Vec::new();
    }
    let mut changed: Vec<_> = module
        .class_fields
        .iter()
        .map(|f| vec![false; f.len()])
        .collect();
    for (f, method) in module.functions.iter().zip(&methods) {
        if let (Type::Class(class), Some(fields)) = (f.returns, method) {
            for (field, value) in changed[class].iter_mut().zip(fields) {
                *field |= value.is_some();
            }
        }
    }
    let readers = module
        .functions
        .iter()
        .map(|f| {
            let &[slot] = f.params.as_slice() else {
                return None;
            };
            let Type::Class(class) = f.locals[slot] else {
                return None;
            };
            module.class_metadata[class].frozen_initializer?;
            Reader::new(f, &changed[class])
        })
        .collect();
    let mut copies = Copies {
        methods,
        readers,
        original_count: module.functions.len(),
        class_fields: &module.class_fields,
        origins: Vec::new(),
    };
    for function in module.functions.iter_mut().chain([&mut module.entry]) {
        let mut reads = vec![0; function.locals.len()];
        let mut writes = vec![0; reads.len()];
        count_writes(&function.body, &mut writes);
        for &p in &function.params {
            writes[p] += 1;
        }
        ir::walk_expressions(&function.body, &mut |v| {
            if let ExprKind::Local(s) = v.kind {
                reads[s] += 1;
            }
        });
        let private: Vec<_> = reads
            .iter()
            .zip(&writes)
            .map(|(&r, &w)| r == 1 && w == 1)
            .collect();
        copies.block(&mut function.body, &private, &mut function.locals);
        ir::walk_expressions_mut(&mut function.body, &mut |expr| copies.direct(expr));
    }
    for &id in &copies.origins {
        let original = &module.functions[id];
        let Type::Class(class) = original.locals[original.params[0]] else {
            unreachable!()
        };
        let variant = copies.readers[id]
            .as_ref()
            .unwrap()
            .specialize(original, &module.class_fields[class]);
        module.functions.push(variant);
    }
    copies.origins
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconstructible_copies_only_disappear_for_a_nonescaping_reader() {
        let source = include_str!("../../../tests/fixtures/dataclasses/copy_readers.py");
        let graph = crate::modules::SourceGraph::single(source).unwrap();
        let mut module = crate::frontend::lower(&graph).unwrap();
        let copy_ids: Vec<_> = module
            .functions
            .iter()
            .enumerate()
            .filter_map(|(id, f)| copy(f, &module).map(|_| id))
            .collect();
        assert_eq!(copy_ids.len(), 3, "on, off, and blank are pure copies");
        let calls = |module: &Module| {
            let mut count = 0;
            for f in &module.functions {
                ir::walk_expressions(&f.body, &mut |e| {
                    if let ExprKind::Call(id, _) = e.kind
                        && copy_ids.contains(&id)
                    {
                        count += 1;
                    }
                });
            }
            count
        };
        let before = calls(&module);
        let variants = scalarize(&mut module);
        assert_eq!(
            variants.len(),
            1,
            "both call sites share the reader variant"
        );
        assert_eq!(
            before - calls(&module),
            8,
            "conditional arms, direct and named copy chains, and the field projection"
        );
        assert_eq!(
            calls(&module),
            2,
            "the list escape and two-use local retain their copies"
        );
    }
}
