//! Fuse two adjacent, fully checked statements when their only observable work
//! is visiting distinct sorted keys and assigning fields of generated records.

use super::*;

impl Checker<'_> {
    pub(super) fn fuse_sorted_rows(&self, source: &[ast::Stmt], body: &mut Vec<Statement>) {
        let Some((builtin, inputs, result)) = self.sorted_rows(source, body) else {
            return;
        };
        body.truncate(body.len() - 2);
        body.push(Statement::Return(Expr {
            ty: result,
            kind: ExprKind::Builtin(builtin, inputs),
        }));
    }

    fn sorted_rows(
        &self,
        source: &[ast::Stmt],
        body: &[Statement],
    ) -> Option<(Builtin, Vec<Expr>, Type)> {
        // Only a function's terminal top-level return qualifies. An enclosing
        // handler or finally could still observe the original keys binding.
        let [
            ..,
            Statement::Assign(keys, sorted),
            Statement::Return(result),
        ] = body
        else {
            return None;
        };
        let ExprKind::Builtin(Builtin::SortedStringKeys, inputs) = &sorted.kind else {
            return None;
        };
        if !matches!(result.kind, ExprKind::Call(..))
            || !inputs.iter().all(|d| self.types.is_default_list_dict(d.ty))
        {
            return None;
        }
        let [.., ast::Stmt::Return(ret)] = source else {
            return None;
        };
        let ast::Expr::ListComp(display) = ret.value.as_deref()? else {
            return None;
        };
        let [clause] = display.generators.as_slice() else {
            return None;
        };
        let (ast::Expr::Name(iter), ast::Expr::Name(target)) = (&clause.iter, &clause.target)
        else {
            return None;
        };
        if clause.is_async
            || !clause.ifs.is_empty()
            || self.names.get(iter.id.as_str()) != Some(keys)
        {
            return None;
        }
        let ast::Expr::Call(call) = &*display.elt else {
            return None;
        };
        // A local callable or an induction variable can shadow an imported class.
        let mut root = &*call.func;
        while let ast::Expr::Attribute(attr) = root {
            root = &attr.value;
        }
        let ast::Expr::Name(root) = root else {
            return None;
        };
        if root.id == target.id
            || self.names.contains_key(root.id.as_str())
            || self.local_functions.contains_key(root.id.as_str())
        {
            return None;
        }
        let class = self.class_reference(&call.func)?;
        let declaration = &self.classes.declarations.definitions[class];
        if !declaration.generated_initializer
            || declaration
                .methods
                .iter()
                .any(|method| method.name.as_str() == "__post_init__")
            || self.types.element(result.ty) != Some(Type::Class(class))
        {
            return None;
        }

        let args = &call.arguments;
        if args.args.len() + args.keywords.len() != declaration.fields.len() {
            return None;
        }
        // Every field is supplied in declaration order. No default factory,
        // arbitrary expression, callback, field conversion or user initializer
        // can intervene between the original subscription and field store.
        let mut columns = Vec::with_capacity(declaration.fields.len());
        for (i, field) in declaration.fields.iter().enumerate() {
            let value = if let Some(arg) = args.args.get(i) {
                if field.kw_only {
                    return None;
                }
                arg
            } else {
                let kw = &args.keywords[i - args.args.len()];
                if kw.arg.as_deref() != Some(field.name.as_str()) {
                    return None;
                }
                &kw.value
            };
            let (column, ty) = match value {
                ast::Expr::Name(name) if name.id == target.id => (0, Type::Str),
                ast::Expr::Subscript(index) => {
                    let (ast::Expr::Name(map), ast::Expr::Name(key)) =
                        (&*index.value, &*index.slice)
                    else {
                        return None;
                    };
                    if map.id == target.id || key.id != target.id {
                        return None;
                    }
                    let slot = self.names.get(map.id.as_str())?;
                    let pos = inputs
                        .iter()
                        .position(|d| matches!(d.kind, ExprKind::Local(other) if other == *slot))?;
                    (
                        u8::try_from(pos + 1).ok()?,
                        self.types.mapping(inputs[pos].ty)?.1,
                    )
                }
                _ => return None,
            };
            if self.classes.fields[class][i] != ty {
                return None;
            }
            columns.push(column);
        }
        if columns.iter().all(|column| *column == 0) {
            return None;
        }
        Some((
            Builtin::SortedRecordRows { class, columns },
            inputs.clone(),
            result.ty,
        ))
    }
}
