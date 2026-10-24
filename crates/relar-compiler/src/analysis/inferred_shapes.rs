use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use ruff_python_ast::visitor::{Visitor, walk_body, walk_expr, walk_stmt};
use ruff_python_ast::{Comprehension, Expr, Number, Stmt, UnaryOp};

const MAX_DEPTH: usize = 256;

/// A flat series of assignments can construct a type much deeper than any one
/// expression. ty recursively maps inferred container types at generic calls.
pub(super) fn check(body: &[Stmt], tokens: usize) -> Result<(), &'static str> {
    if tokens <= MAX_DEPTH {
        return Ok(());
    }
    let mut shapes = Shapes {
        scopes: vec![HashMap::new()],
        straight_line: HashMap::new(),
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

#[derive(Clone, Default)]
struct Shape {
    depth: usize,
    contents: Contents,
}

#[derive(Clone, Default)]
enum Contents {
    #[default]
    Unknown,
    List,
    Dict {
        key_depth: usize,
    },
    UniformTuple(usize),
    Tuple(Rc<[Shape]>),
}

impl Shape {
    fn plain(depth: usize) -> Self {
        Self {
            depth,
            contents: Contents::Unknown,
        }
    }

    fn list(depth: usize) -> Self {
        Self {
            depth,
            contents: Contents::List,
        }
    }

    fn iteration_depth(&self) -> usize {
        match self.contents {
            Contents::Dict { key_depth } => key_depth,
            _ => self.depth.saturating_sub(1),
        }
    }

    fn captured_depth(&self, prefix: usize, suffix: usize) -> usize {
        match &self.contents {
            Contents::Tuple(fields) => {
                if let Some(captured) = fields.get(prefix..fields.len().saturating_sub(suffix)) {
                    return captured.iter().map(|field| field.depth).max().unwrap_or(0);
                }
            }
            Contents::UniformTuple(length) if prefix + suffix == *length => return 0,
            _ => {}
        }
        self.depth.saturating_sub(1)
    }

    fn element(&self, index: Option<isize>) -> Self {
        if let (Contents::Tuple(fields), Some(index)) = (&self.contents, index) {
            let position = if index < 0 {
                fields.len().checked_add_signed(index)
            } else {
                Some(index as usize)
            };
            if let Some(field) = position.and_then(|position| fields.get(position)) {
                return field.clone();
            }
        }
        Self::plain(self.depth.saturating_sub(1))
    }
}

fn literal_index(expr: &Expr) -> Option<isize> {
    let (expr, negative) = match expr {
        Expr::UnaryOp(node) if node.op == UnaryOp::USub => (&*node.operand, true),
        expr => (expr, false),
    };
    let Expr::NumberLiteral(node) = expr else {
        return None;
    };
    let Number::Int(value) = &node.value else {
        return None;
    };
    let index = isize::try_from(value.as_i64()?).ok()?;
    if negative {
        index.checked_neg()
    } else {
        Some(index)
    }
}

fn bind_target<'a>(target: &'a Expr, shape: Shape, bind: &mut impl FnMut(&'a str, Shape)) {
    let elements = match target {
        Expr::Name(node) => return bind(node.id.as_str(), shape),
        // A starred target receives a list of the remaining elements.
        Expr::Starred(node) => {
            return bind_target(&node.value, Shape::list(shape.depth + 1), bind);
        }
        Expr::Tuple(node) => &node.elts,
        Expr::List(node) => &node.elts,
        _ => return,
    };
    let star = elements
        .iter()
        .position(|elt| matches!(elt, Expr::Starred(_)));
    for (index, element) in elements.iter().enumerate() {
        let field = match star {
            Some(star) if index == star => {
                Shape::plain(shape.captured_depth(index, elements.len() - index - 1))
            }
            Some(star) if index > star => {
                shape.element(Some(index as isize - elements.len() as isize))
            }
            _ => shape.element(Some(index as isize)),
        };
        bind_target(element, field, bind);
    }
}

struct Shapes<'a> {
    fixed: HashSet<&'a str>,
    scopes: Vec<HashMap<&'a str, Shape>>,
    straight_line: HashMap<&'a str, Shape>,
    conditional: bool,
    exceeded: bool,
}

impl<'a> Shapes<'a> {
    fn scope_binding(&self, name: &str) -> Option<&Shape> {
        self.scopes.iter().rev().find_map(|s| s.get(name))
    }

    fn lookup(&self, name: &str) -> Shape {
        self.straight_line
            .get(name)
            .or_else(|| self.scope_binding(name))
            .cloned()
            .unwrap_or_default()
    }

    fn shape(&self, value: &Expr, locals: &[(&str, Shape)]) -> Shape {
        match value {
            Expr::Name(node) => locals
                .iter()
                .rev()
                .find(|(name, _)| *name == node.id.as_str())
                .map_or_else(|| self.lookup(node.id.as_str()), |(_, shape)| shape.clone()),
            Expr::Tuple(node) if !node.elts.iter().any(|elt| matches!(elt, Expr::Starred(_))) => {
                let fields: Vec<_> = node
                    .elts
                    .iter()
                    .map(|elt| self.shape(elt, locals))
                    .collect();
                let deepest = fields.iter().map(|field| field.depth).max().unwrap_or(0);
                // Uniform fields need only their arity to recognize an empty capture.
                let distinct = fields.iter().any(|field| {
                    field.depth != deepest || !matches!(field.contents, Contents::Unknown)
                });
                Shape {
                    depth: deepest + 1,
                    contents: if distinct {
                        Contents::Tuple(fields.into())
                    } else {
                        Contents::UniformTuple(fields.len())
                    },
                }
            }
            Expr::Dict(node) => {
                let mut depth = 1;
                let mut key_depth = 0;
                for item in &node.items {
                    if let Some(key) = &item.key {
                        let key = self.depth_in(key, locals);
                        key_depth = key_depth.max(key);
                        depth = depth.max(1 + key.max(self.depth_in(&item.value, locals)));
                    } else {
                        let mapping = self.shape(&item.value, locals);
                        depth = depth.max(mapping.depth);
                        key_depth = key_depth.max(mapping.iteration_depth());
                    }
                }
                Shape {
                    depth,
                    contents: Contents::Dict { key_depth },
                }
            }
            Expr::DictComp(node) => {
                let bindings = self.comprehension_bindings(&node.generators, locals);
                let key_depth =
                    self.depth_in(node.key.as_deref().unwrap_or(&node.value), &bindings);
                let depth = 1 + key_depth.max(self.depth_in(&node.value, &bindings));
                Shape {
                    depth,
                    contents: Contents::Dict { key_depth },
                }
            }
            Expr::List(_) | Expr::ListComp(_) => Shape::list(self.depth_in(value, locals)),
            Expr::Subscript(node) if !matches!(&*node.slice, Expr::Slice(_)) => self
                .shape(&node.value, locals)
                .element(literal_index(&node.slice)),
            Expr::Named(node) => self.shape(&node.value, locals),
            _ => Shape::plain(self.depth_in(value, locals)),
        }
    }

    fn iteration_bindings<'v>(
        &self,
        target: &'v Expr,
        iter: &'v Expr,
        locals: &[(&'v str, Shape)],
    ) -> (usize, HashMap<&'v str, Shape>) {
        let bindings = |item: Shape| {
            let depth = item.depth;
            let mut names = HashMap::new();
            bind_target(target, item, &mut |name, shape| {
                names.insert(name, shape);
            });
            (depth, names)
        };
        // For a literal iterable, bind each possible row before joining its
        // names. A repeated name within a row takes its last assignment.
        if let Expr::List(list) = iter
            && matches!(target, Expr::Tuple(_) | Expr::List(_))
            && let Some((first, rest)) = list.elts.split_first()
            && !list.elts.iter().any(|elt| matches!(elt, Expr::Starred(_)))
        {
            let (mut deepest, mut names) = bindings(self.shape(first, locals));
            for row in rest {
                let (depth, fields) = bindings(self.shape(row, locals));
                deepest = deepest.max(depth);
                for (name, field) in fields {
                    // Every row is destructured by the same target.
                    if let Some(old) = names.get_mut(name) {
                        let depth = old.depth.max(field.depth);
                        *old = if matches!(
                            (&old.contents, &field.contents),
                            (Contents::List, Contents::List)
                        ) {
                            Shape::list(depth)
                        } else {
                            Shape::plain(depth)
                        };
                    }
                }
            }
            (deepest, names)
        } else {
            bindings(Shape::plain(self.shape(iter, locals).iteration_depth()))
        }
    }

    fn comprehension_bindings<'v>(
        &self,
        generators: &'v [Comprehension],
        locals: &[(&'v str, Shape)],
    ) -> Vec<(&'v str, Shape)> {
        let mut bindings = locals.to_vec();
        for gen_ in generators {
            let (_, names) = self.iteration_bindings(&gen_.target, &gen_.iter, &bindings);
            bindings.extend(names);
        }
        bindings
    }

    fn comprehension<'v>(
        &self,
        generators: &'v [Comprehension],
        values: &[&'v Expr],
        locals: &[(&'v str, Shape)],
    ) -> usize {
        let bindings = self.comprehension_bindings(generators, locals);
        1 + values
            .iter()
            .map(|e| self.depth_in(e, &bindings))
            .max()
            .unwrap_or(0)
    }

    fn depth_in<'v>(&self, value: &'v Expr, locals: &[(&'v str, Shape)]) -> usize {
        let rank = |e: &'v Expr| self.depth_in(e, locals);
        let max_depth = |items: &'v [Expr]| items.iter().map(rank).max().unwrap_or(0);
        match value {
            Expr::Name(_) => self.shape(value, locals).depth,
            Expr::Tuple(node) => 1 + max_depth(&node.elts),
            Expr::List(node) => 1 + max_depth(&node.elts),
            Expr::Set(node) => 1 + max_depth(&node.elts),
            Expr::Dict(_) | Expr::DictComp(_) => self.shape(value, locals).depth,
            Expr::BoolOp(node) => max_depth(&node.values),
            Expr::If(node) => rank(&node.body).max(rank(&node.orelse)),
            Expr::Named(node) => rank(&node.value),
            Expr::Starred(node) => rank(&node.value).saturating_sub(1),
            Expr::Subscript(node) if matches!(&*node.slice, Expr::Slice(_)) => rank(&node.value),
            Expr::Subscript(_) => self.shape(value, locals).depth,
            Expr::ListComp(node) => self.comprehension(&node.generators, &[&node.elt], locals),
            Expr::SetComp(node) => self.comprehension(&node.generators, &[&node.elt], locals),
            Expr::Generator(node) => self.comprehension(&node.generators, &[&node.elt], locals),
            Expr::Call(node) => {
                if let Some(attr) = node.func.as_attribute_expr()
                    && attr.attr.as_str() == "pop"
                {
                    let receiver = self.shape(&attr.value, locals);
                    if matches!(receiver.contents, Contents::List) {
                        return receiver.depth.saturating_sub(1);
                    }
                }
                let name = node.func.as_name_expr().map(|name| name.id.as_str());
                let args = &node.arguments.args;
                let item_depth = |arg: &Expr| self.shape(arg, locals).iteration_depth();
                match name {
                    // Both the iterator and the tuple around its items add a level.
                    Some("zip") => 2 + args.iter().map(item_depth).max().unwrap_or(0),
                    Some("enumerate") => 2 + args.first().map_or(0, item_depth),
                    Some(
                        "tuple" | "list" | "set" | "frozenset" | "sorted" | "iter" | "reversed",
                    ) => 1 + args.first().map_or(0, item_depth),
                    Some("dict") => args.first().map_or(1, |e| rank(e).max(1)),
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

    fn bind(&mut self, target: &'a Expr, shape: Shape, definite: bool) {
        self.exceeded |= shape.depth > MAX_DEPTH;
        bind_target(target, shape, &mut |name, shape| {
            self.bind_name(name, shape, definite)
        });
    }

    fn bind_name(&mut self, name: &'a str, shape: Shape, definite: bool) {
        self.exceeded |= shape.depth > MAX_DEPTH;
        let old = self.scope_binding(name).cloned().unwrap_or_default();
        if self.conditional {
            if definite && (old.depth != 0 || shape.depth != 0) {
                self.straight_line.insert(name, shape.clone());
            } else {
                self.straight_line.remove(name);
            }
        }
        if shape.depth != 0 || old.depth != 0 {
            self.scopes.last_mut().unwrap().insert(
                name,
                if self.conditional {
                    let depth = old.depth.max(shape.depth);
                    if matches!(
                        (&old.contents, &shape.contents),
                        (Contents::List, Contents::List)
                    ) {
                        Shape::list(depth)
                    } else {
                        Shape::plain(depth)
                    }
                } else {
                    shape
                },
            );
        }
    }
}

impl<'a> Visitor<'a> for Shapes<'a> {
    fn visit_body(&mut self, body: &'a [Stmt]) {
        // Within an arm, a completed assignment is definite for the next
        // statement. Across arms use the conservative facts in the scope.
        self.straight_line.clear();
        walk_body(self, body);
        self.straight_line.clear();
    }

    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        if self.exceeded {
            return;
        }
        match stmt {
            Stmt::Assign(node) => {
                self.visit_expr(&node.value);
                let shape = self.shape(&node.value, &[]);
                for target in &node.targets {
                    self.bind(target, shape.clone(), true);
                }
            }
            Stmt::AnnAssign(node) => {
                if let Some(value) = &node.value {
                    self.visit_expr(value);
                    self.bind(&node.target, self.shape(value, &[]), true);
                }
            }
            Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {
                self.straight_line.clear();
                self.scopes.push(HashMap::new());
                let outer = std::mem::take(&mut self.conditional);
                if let Stmt::FunctionDef(function) = stmt {
                    for parameter in function.parameters.iter() {
                        let name = parameter.name().as_str();
                        if self.lookup(name).depth != 0 {
                            self.scopes
                                .last_mut()
                                .unwrap()
                                .insert(name, Shape::default());
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
                    let (depth, names) = self.iteration_bindings(&node.target, &node.iter, &[]);
                    self.exceeded |= depth > MAX_DEPTH;
                    for (name, shape) in names {
                        self.bind_name(name, shape, false);
                    }
                }
                walk_stmt(self, stmt);
                self.conditional = outer;
            }
        }
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        walk_expr(self, expr);
        if let Expr::Named(node) = expr {
            self.bind(&node.target, self.shape(&node.value, &[]), false);
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
    fn bounds_tuples_yielded_by_zip_and_enumerate() {
        for call in ["zip(input)", "zip([0], input)", "enumerate(input, 42)"] {
            let mut body = String::from("v0 = [1]\n");
            for i in 1..=MAX_DEPTH {
                if i == MAX_DEPTH {
                    assert!(!limited(&body), "{call}");
                }
                let wrapper = call.replace("input", &format!("v{}", i - 1));
                body.push_str(&format!("v{i} = list({wrapper})\n"));
            }
            assert!(limited(&body), "{call}");
        }
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
    fn dictionary_iteration_counts_only_key_nesting() {
        for step in [
            "x = list(zip({0: x}))",
            "d = {0: x}\nalias = d\nx = list(enumerate(alias))",
            "d = {0: x}\nx = list(zip([0], {**d}))",
            "x = list(enumerate({k: x for k in [0]}))",
        ] {
            assert!(
                !limited(&format!("x = [1]\n{}", format!("{step}\n").repeat(300))),
                "{step}"
            );
        }
        let deep = format!("x = 0\n{}", "x = (x,)\n".repeat(MAX_DEPTH - 2));
        assert!(!limited(&format!(
            "{deep}d = {{x: 0}}\nitems = list(zip(d))\n"
        )));
        assert!(limited(&format!(
            "{deep}d = {{x: 0}}\nitems = (list(zip(d)),)\n"
        )));
        assert!(limited(&format!(
            "{deep}d = {{0: x}}\ncopy = ((dict(d),),)\n"
        )));
    }

    #[test]
    fn bounds_nesting_through_unpack_targets() {
        for step in [
            "*v{i}, = (v{p},)",
            "v{i} = [rest for *rest, in [(v{p},)]][0]",
            "v{i} = [(item,) for (item,) in [(v{p},)]][0]",
        ] {
            let mut body = String::from("v0 = (1,)\n");
            for i in 1..=MAX_DEPTH {
                if i == MAX_DEPTH {
                    assert!(!limited(&body), "{step}");
                }
                body.push_str(
                    &step
                        .replace("{i}", &i.to_string())
                        .replace("{p}", &(i - 1).to_string()),
                );
                body.push('\n');
            }
            assert!(limited(&body), "{step}");
        }
    }

    #[test]
    fn tuple_indices_and_unpacking_keep_the_selected_field_depth() {
        for step in [
            "x = ((x, 1)[1],)",
            "pair = (x, 1)\nalias = pair\nx = (alias[-1],)",
            "pair = ((x, 1),)\nx = (pair[0][1],)",
            "pair = (x, 1)\nignored, item = pair\nx = (item,)",
        ] {
            let mut body = String::from("x = (1,)\n");
            for _ in 0..300 {
                body.push_str(step);
                body.push('\n');
            }
            assert!(!limited(&body), "{step}");
        }
        let mut deep = String::from("x = (1,)\n");
        deep.push_str(&"x = (x,)\n".repeat(MAX_DEPTH - 2));
        deep.push_str("pair = (x, 1)\nx = 1\n");
        assert!(!limited(&format!("{deep}result = ((pair[1],),)\n")));
        assert!(limited(&format!("{deep}result = ((pair[0],),)\n")));
        assert!(limited(&format!("{deep}result = ((pair[index],),)\n")));
        let arms = "if flag:\n    y = pair[0]\nelse:\n    y = pair[1]\n";
        assert!(limited(&format!("{deep}{arms}result = ((y,),)\n")));
        let in_arm = format!(
            "x = (1,)\nif flag:\n{}",
            "    pair = (x, 1)\n    x = (pair[1],)\n".repeat(300)
        );
        assert!(!limited(&in_arm));
    }

    #[test]
    fn popping_from_a_known_list_returns_an_element() {
        for step in [
            "x = [x.pop()]",
            "alias = x\nx = [alias.pop()]",
            "pair = (x, 0)\nx = [pair[0].pop()]",
        ] {
            let mut body = String::from("x = [1]\n");
            for _ in 0..300 {
                body.push_str(step);
                body.push('\n');
            }
            assert!(!limited(&body), "{step}");
        }
        let deep = format!("x = 1\n{}", "x = [x]\n".repeat(MAX_DEPTH));
        let in_arm = format!(
            "x = [1]\nif flag:\n{}",
            "    alias = x\n    x = [alias.pop()]\n".repeat(300)
        );
        assert!(!limited(&in_arm));
        assert!(!limited(&format!("{deep}result = [x.pop()]\n")));
        assert!(limited(&format!("{deep}result = [[x.pop()]]\n")));
        assert!(limited(&format!("{deep}result = receiver.pop((x,))\n")));
    }

    #[test]
    fn starred_targets_use_only_the_captured_tuple_fields() {
        for step in [
            "*x, tail = (1, x)",
            "head, *x = (x, 1)",
            "head, *x, tail = (x, 1, x)",
            "*x, tail = (x,)",
            "pair = (1, x)\nalias = pair\n*x, tail = alias",
            "x = [rest for *rest, tail in [(1, x), (2, x)]][0]",
            "x = [(item,) for item, *rest, item in [(x, 0, 1)]][0]",
        ] {
            assert!(
                !limited(&format!("x = [1]\n{}", format!("{step}\n").repeat(300))),
                "{step}"
            );
        }
        let deep = format!("x = 1\n{}", "x = (x,)\n".repeat(MAX_DEPTH - 1));
        assert!(!limited(&format!(
            "{deep}*empty, tail = (x,)\nresult = (empty,)\n"
        )));
        assert!(limited(&format!(
            "{deep}*captured, tail = (x, 1)\nresult = (captured,)\n"
        )));
        assert!(limited(&format!(
            "{deep}head, *rest, tail = (1, 1, x)\nresult = ((tail,),)\n"
        )));
        assert!(limited(&format!(
            "{deep}picked = [rest for *rest, tail in [(1, 1), (x, 1)]][0]\nresult = (picked,)\n"
        )));
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
