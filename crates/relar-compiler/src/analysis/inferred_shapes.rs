use std::collections::{HashMap, HashSet};

use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};
use ruff_python_ast::{Comprehension, Expr, Stmt};

const MAX_DEPTH: usize = 256;

/// A flat series of assignments can construct a type much deeper than any one
/// expression. ty recursively maps inferred container types at generic calls.
pub(super) fn check(body: &[Stmt], tokens: usize) -> Result<(), &'static str> {
    if tokens <= MAX_DEPTH {
        return Ok(());
    }
    let mut shapes = Shapes {
        scopes: vec![HashMap::new()],
        fixed: body
            .iter()
            .filter_map(|stmt| match stmt {
                Stmt::ClassDef(class) => Some(class.name.as_str()),
                Stmt::FunctionDef(function)
                    if function.returns.as_deref().is_some_and(scalar_annotation) =>
                {
                    Some(function.name.as_str())
                }
                _ => None,
            })
            .collect(),
        conditional: false,
        exceeded: false,
    };
    shapes.visit_body(body);
    if shapes.exceeded {
        Err("Relar: inferred container nesting exceeds 256 levels")
    } else {
        Ok(())
    }
}

fn scalar_annotation(expr: &Expr) -> bool {
    matches!(expr, Expr::NoneLiteral(_))
        || matches!(expr, Expr::Name(name) if matches!(name.id.as_str(), "str" | "bytes" | "int" | "bool" | "float"))
}

struct Shapes<'a> {
    fixed: HashSet<&'a str>,
    scopes: Vec<HashMap<&'a str, usize>>,
    conditional: bool,
    exceeded: bool,
}

impl<'a> Shapes<'a> {
    fn lookup(&self, name: &str) -> usize {
        self.scopes
            .iter()
            .rev()
            .find_map(|s| s.get(name))
            .copied()
            .unwrap_or(0)
    }

    fn depth(&self, value: &Expr) -> usize {
        self.depth_in(value, &[])
    }

    fn comprehension<'v>(
        &self,
        generators: &'v [Comprehension],
        values: &[&'v Expr],
        locals: &[(&'v str, usize)],
    ) -> usize {
        let mut bindings = locals.to_vec();
        for gen_ in generators {
            let depth = self.depth_in(&gen_.iter, &bindings).saturating_sub(1);
            if let Expr::Name(target) = &gen_.target {
                bindings.push((target.id.as_str(), depth));
            }
        }
        1 + values
            .iter()
            .map(|e| self.depth_in(e, &bindings))
            .max()
            .unwrap_or(0)
    }

    fn depth_in<'v>(&self, value: &'v Expr, locals: &[(&'v str, usize)]) -> usize {
        let rank = |e: &'v Expr| self.depth_in(e, locals);
        let max_depth = |items: &'v [Expr]| items.iter().map(rank).max().unwrap_or(0);
        match value {
            Expr::Name(node) => locals
                .iter()
                .rev()
                .find(|(name, _)| *name == node.id.as_str())
                .map_or_else(|| self.lookup(node.id.as_str()), |(_, d)| *d),
            Expr::Tuple(node) => 1 + max_depth(&node.elts),
            Expr::List(node) => 1 + max_depth(&node.elts),
            Expr::Set(node) => 1 + max_depth(&node.elts),
            Expr::Dict(node) => node
                .items
                .iter()
                .map(|item| {
                    let value_depth = rank(&item.value);
                    item.key
                        .as_ref()
                        .map_or(value_depth, |key| 1 + value_depth.max(rank(key)))
                })
                .max()
                .unwrap_or(1),
            Expr::BoolOp(node) => max_depth(&node.values),
            Expr::If(node) => rank(&node.body).max(rank(&node.orelse)),
            Expr::Named(node) => rank(&node.value),
            Expr::Starred(node) => rank(&node.value).saturating_sub(1),
            Expr::Subscript(node) if matches!(&*node.slice, Expr::Slice(_)) => rank(&node.value),
            Expr::Subscript(node) => rank(&node.value).saturating_sub(1),
            Expr::ListComp(node) => self.comprehension(&node.generators, &[&node.elt], locals),
            Expr::SetComp(node) => self.comprehension(&node.generators, &[&node.elt], locals),
            Expr::Generator(node) => self.comprehension(&node.generators, &[&node.elt], locals),
            Expr::DictComp(node) => self.comprehension(
                &node.generators,
                &[node.key.as_deref().unwrap_or(&node.value), &node.value],
                locals,
            ),
            Expr::Call(node) => {
                let name = node.func.as_name_expr().map(|name| name.id.as_str());
                let args = &node.arguments.args;
                match name {
                    Some(
                        "tuple" | "list" | "set" | "frozenset" | "dict" | "sorted" | "iter"
                        | "reversed",
                    ) => args.first().map_or(1, |e| rank(e).max(1)),
                    Some("next") => args.first().map_or(0, |e| rank(e).saturating_sub(1)),
                    Some("len" | "int" | "float" | "str" | "bool" | "bytes" | "sum" | "print") => 0,
                    Some(name) if self.fixed.contains(name) => 0,
                    _ => max_depth(args)
                        .max(
                            node.func
                                .as_attribute_expr()
                                .map_or(0, |attr| rank(&attr.value)),
                        )
                        .max(
                            node.arguments
                                .keywords
                                .iter()
                                .map(|kw| rank(&kw.value))
                                .max()
                                .unwrap_or(0),
                        ),
                }
            }
            Expr::BinOp(node) => rank(&node.left).max(rank(&node.right)),
            _ => 0,
        }
    }

    fn bind(&mut self, target: &'a Expr, depth: usize) {
        self.exceeded |= depth > MAX_DEPTH;
        match target {
            Expr::Name(node) => {
                let name = node.id.as_str();
                let old = self.lookup(name);
                if depth != 0 || old != 0 {
                    self.scopes.last_mut().unwrap().insert(
                        name,
                        if self.conditional {
                            old.max(depth)
                        } else {
                            depth
                        },
                    );
                }
            }
            Expr::List(node) => node
                .elts
                .iter()
                .for_each(|e| self.bind(e, depth.saturating_sub(1))),
            Expr::Tuple(node) => node
                .elts
                .iter()
                .for_each(|e| self.bind(e, depth.saturating_sub(1))),
            _ => {}
        }
    }
}

impl<'a> Visitor<'a> for Shapes<'a> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        if self.exceeded {
            return;
        }
        match stmt {
            Stmt::Assign(node) => {
                self.visit_expr(&node.value);
                let depth = self.depth(&node.value);
                for target in &node.targets {
                    self.bind(target, depth);
                }
            }
            Stmt::AnnAssign(node) => {
                if let Some(value) = &node.value {
                    self.visit_expr(value);
                    let depth = self.depth(value);
                    self.bind(&node.target, depth);
                }
            }
            Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {
                self.scopes.push(HashMap::new());
                let outer = std::mem::take(&mut self.conditional);
                if let Stmt::FunctionDef(function) = stmt {
                    for parameter in function.parameters.iter() {
                        let name = parameter.name().as_str();
                        if self.lookup(name) != 0 {
                            self.scopes.last_mut().unwrap().insert(name, 0);
                        }
                    }
                }
                walk_stmt(self, stmt);
                self.conditional = outer;
                self.scopes.pop();
            }
            _ => {
                let outer = self.conditional;
                self.conditional |= matches!(
                    stmt,
                    Stmt::If(_) | Stmt::For(_) | Stmt::While(_) | Stmt::Match(_) | Stmt::Try(_)
                );
                if let Stmt::For(node) = stmt {
                    self.bind(&node.target, self.depth(&node.iter).saturating_sub(1));
                }
                walk_stmt(self, stmt);
                self.conditional = outer;
            }
        }
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        walk_expr(self, expr);
        if let Expr::Named(node) = expr {
            self.bind(&node.target, self.depth(&node.value));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ruff_python_parser::parse_module;

    fn limited(body: &str) -> bool {
        let parsed = parse_module(body).unwrap();
        check(&parsed.syntax().body, parsed.tokens().len()).is_err()
    }

    #[test]
    fn bounds_flat_wrapping_and_aliases() {
        let mut body = String::from("v0 = (1,)\n");
        for i in 1..MAX_DEPTH {
            body.push_str(&format!("alias = v{}\nv{i} = (alias,)\n", i - 1));
        }
        assert!(!limited(&body));
        let parameter = format!("v{}", MAX_DEPTH - 1);
        assert!(!limited(&format!(
            "{body}def f({parameter}):\n    result = ({parameter},)\n"
        )));
        for wrap in [
            "({p},)",
            "[{p}]",
            "{1: {p}}",
            "tuple(({p},))",
            "({p},)[:]",
            "(alias := ({p},))",
            "({p},) or ()",
            "next(iter((({p},),)))",
            "[(x,) for x in [{p}]][0]",
            "generic_identity(({p},))",
            "({p},) * 1",
            "({p},).__add__(())",
        ] {
            let last = wrap.replace("{p}", &format!("v{}", MAX_DEPTH - 1));
            assert!(limited(&format!("{body}result = {last}\n")), "{wrap}");
        }
    }

    #[test]
    fn independent_and_rebound_names_stay_shallow() {
        assert!(!limited(&"value = 1\nvalue = (value,)\n".repeat(1000)));
        assert!(!limited(
            &(0..1000)
                .map(|i| format!("def f{i}():\n    value = (1,)\n"))
                .collect::<String>()
        ));
    }
}
