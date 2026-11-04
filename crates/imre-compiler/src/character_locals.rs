//! Keep single-character iteration results as code points when their only uses
//! are comparisons and `ord`. Copying or moving a local preserves its value.

use crate::ir::{self, Builtin, Expr, ExprKind, Function, IterNextKind, Module, Statement, Type};

#[derive(Default)]
struct Uses {
    reads: usize,
    handled_reads: usize,
    needs_string: bool,
    links: Vec<usize>,
}

fn link(uses: &mut [Uses], a: usize, b: usize) {
    uses[a].links.push(b);
    uses[b].links.push(a);
}

fn one_character(expr: &Expr) -> Option<i64> {
    let ExprKind::Str(text) = &expr.kind else {
        return None;
    };
    let mut chars = text.chars();
    let ch = chars.next()?;
    chars.next().is_none().then_some(i64::from(ch as u32))
}

/// The scalar result of an eager scope is its value after its body has run.
fn result_slot(expr: &Expr) -> Option<usize> {
    if expr.ty != Type::Str {
        return None;
    }
    match &expr.kind {
        ExprKind::Local(slot) => Some(*slot),
        ExprKind::Scope { value, .. } => result_slot(value),
        _ => None,
    }
}

/// Frame inspection requires the original local types and values.
pub(crate) fn specialize(module: &mut Module) {
    if module.source_frames.is_empty() {
        for function in module.functions.iter_mut().chain([&mut module.entry]) {
            specialize_function(function);
        }
    }
}

fn specialize_function(function: &mut Function) {
    let mut producers = Vec::new();
    ir::walk_statements(&function.body, &mut |stmt| {
        if let Statement::IterNext {
            kind: IterNextKind::String,
            key,
            ..
        } = stmt
        {
            producers.push(*key);
        }
    });
    if producers.is_empty() {
        return;
    }
    let mut uses: Vec<_> = function
        .locals
        .iter()
        .map(|&ty| Uses {
            needs_string: ty != Type::Str,
            ..Uses::default()
        })
        .collect();
    for &param in &function.params {
        uses[param].needs_string = true;
    }
    ir::walk_statements(&function.body, &mut |stmt| match stmt {
        Statement::Assign(target, value) => {
            if let Some(source) = result_slot(value) {
                uses[source].handled_reads += 1;
                link(&mut uses, *target, source);
            } else {
                uses[*target].needs_string = true;
            }
        }
        Statement::Move(target, source) => link(&mut uses, *target, *source),
        Statement::IterNext {
            kind: IterNextKind::Generic,
            key,
            value,
            ..
        } => {
            uses[*key].needs_string = true;
            if let Some(value) = value {
                uses[*value].needs_string = true;
            }
        }
        _ => {}
    });
    ir::walk_expressions(&function.body, &mut |expr| match &expr.kind {
        ExprKind::Local(slot) => uses[*slot].reads += 1,
        ExprKind::Bind(slot, _) => uses[*slot].needs_string = true,
        ExprKind::Builtin(Builtin::Ord, args) => {
            if let [arg] = args.as_slice()
                && let Some(slot) = result_slot(arg)
            {
                uses[slot].handled_reads += 1;
            }
        }
        ExprKind::Compare(values, _)
            if values
                .iter()
                .all(|v| result_slot(v).is_some() || one_character(v).is_some()) =>
        {
            // Unicode scalar order is also lexicographic order for exact
            // one-character strings. Every local in a chain must qualify.
            let mut first = None;
            for slot in values.iter().filter_map(result_slot) {
                uses[slot].handled_reads += 1;
                if let Some(first) = first {
                    link(&mut uses, first, slot);
                } else {
                    first = Some(slot);
                }
            }
        }
        _ => {}
    });

    // A comparison or assignment connects representations in both directions.
    // A single escape keeps every local in its connected component boxed.
    let mut rejected: Vec<_> = uses
        .iter()
        .enumerate()
        .filter_map(|(i, u)| (u.needs_string || u.reads != u.handled_reads).then_some(i))
        .collect();
    let mut blocked = vec![false; uses.len()];
    while let Some(i) = rejected.pop() {
        if !std::mem::replace(&mut blocked[i], true) {
            rejected.extend(&uses[i].links);
        }
    }
    let mut scalars = vec![false; uses.len()];
    while let Some(i) = producers.pop() {
        if !blocked[i] && !std::mem::replace(&mut scalars[i], true) {
            function.locals[i] = Type::Int;
            producers.extend(&uses[i].links);
        }
    }
    if !scalars.iter().any(|&scalar| scalar) {
        return;
    }
    ir::rewrite_statements(&mut function.body, &mut |stmt| {
        if let Statement::IterNext { key, kind, .. } = stmt
            && scalars[*key]
        {
            *kind = IterNextKind::CodePoint;
        }
    });
    ir::walk_expressions_mut(&mut function.body, &mut |expr| match &mut expr.kind {
        ExprKind::Local(slot) if scalars[*slot] => expr.ty = Type::Int,
        ExprKind::Scope { value, .. } if result_slot(value).is_some_and(|i| scalars[i]) => {
            expr.ty = Type::Int;
        }
        ExprKind::Builtin(Builtin::Ord, args) => {
            if let [arg] = args.as_slice()
                && result_slot(arg).is_some_and(|i| scalars[i])
            {
                // Keep argument evaluation, including any eager scope and cleanup.
                *expr = args.pop().expect("ord has one argument");
                expr.ty = Type::Int;
            }
        }
        ExprKind::Compare(values, _)
            if values.iter().filter_map(result_slot).any(|i| scalars[i]) =>
        {
            for value in values {
                if let Some(code) = one_character(value) {
                    *value = Expr::integer(code);
                }
            }
        }
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use crate::{CompileOptions, compile};

    #[test]
    fn scalarizes_character_comparisons_and_ord_but_preserves_escaping_strings() {
        for (body, scalar) in [
            ("score += ord(ch)", true),
            ("alias = ch\n        score += int(alias == '🙂')", true),
            ("score += int(ch == needle)", false),
            ("print(ch)\n        score += ord(ch)", false),
            ("score += int(ch == 'ab')", false),
        ] {
            let source = format!(
                "def scan(text: str, needle: str) -> int:\n    score = 0\n    for ch in text:\n        {body}\n    return score\n"
            );
            let llvm = compile(&source, &CompileOptions::default()).unwrap();
            assert_eq!(
                llvm.contains("call i64 @imre_str_iter_next_codepoint("),
                scalar,
                "{source}"
            );
            assert_eq!(
                llvm.contains("call i64 @imre_str_iter_next_owned("),
                !scalar,
                "{source}"
            );
        }
    }
}
