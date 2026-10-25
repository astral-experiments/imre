use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use ruff_python_ast::visitor::{Visitor, walk_body, walk_expr, walk_stmt};
use ruff_python_ast::{Comprehension, Expr, Number, Stmt, UnaryOp};
use ruff_text_size::TextSize;

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
        read_shapes: HashMap::new(),
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
        Err("Imre: inferred container nesting exceeds 256 levels")
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
    List(usize),
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

    fn list(depth: usize, layers: usize) -> Self {
        Self {
            depth,
            contents: Contents::List(layers),
        }
    }

    fn list_layers(&self) -> usize {
        match self.contents {
            Contents::List(layers) => layers,
            _ => 0,
        }
    }

    fn item_list_layers(&self) -> usize {
        match &self.contents {
            Contents::Tuple(fields) => fields.iter().map(Self::list_layers).min().unwrap_or(0),
            _ => self.list_layers().saturating_sub(1),
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
        let depth = self.depth.saturating_sub(1);
        if let Contents::List(layers) = self.contents
            && layers > 1
        {
            Self::list(depth, layers - 1)
        } else {
            Self::plain(depth)
        }
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
            return bind_target(&node.value, Shape::list(shape.depth + 1, 1), bind);
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
    read_shapes: HashMap<TextSize, Shape>,
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
                .map(|(_, shape)| shape)
                .or_else(|| self.read_shapes.get(&node.range.start()))
                .cloned()
                .unwrap_or_else(|| self.lookup(node.id.as_str())),
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
            Expr::List(node) => {
                let mut depth = 0;
                let mut common = None::<usize>;
                for item in &node.elts {
                    let item = self.shape(item, locals);
                    depth = depth.max(item.depth);
                    let layers = item.list_layers();
                    common = Some(common.map_or(layers, |old| old.min(layers)));
                }
                Shape::list(depth + 1, common.unwrap_or(0) + 1)
            }
            Expr::ListComp(_) => Shape::list(self.depth_in(value, locals), 1),
            Expr::Call(node) => {
                let rank = |arg: &Expr| self.depth_in(arg, locals);
                let max_depth = |args: &[Expr]| args.iter().map(rank).max().unwrap_or(0);
                let name = node.func.as_name_expr().map(|name| name.id.as_str());
                let args = &node.arguments.args;
                let unknown_call_depth = |receiver| {
                    max_depth(args).max(receiver).max(
                        node.arguments
                            .keywords
                            .iter()
                            .map(|kw| rank(&kw.value))
                            .max()
                            .unwrap_or(0),
                    )
                };
                if let Some(attr) = node.func.as_attribute_expr()
                    && attr.attr.as_str() == "pop"
                {
                    let receiver = self.shape(&attr.value, locals);
                    return if receiver.list_layers() > 0 {
                        receiver.element(None)
                    } else {
                        Shape::plain(unknown_call_depth(receiver.depth))
                    };
                }
                if matches!(name, Some("list" | "sorted")) {
                    let input = args
                        .first()
                        .map(|arg| self.shape(arg, locals))
                        .unwrap_or_default();
                    return Shape::list(1 + input.iteration_depth(), input.item_list_layers() + 1);
                }
                let item_depth = |arg: &Expr| self.shape(arg, locals).iteration_depth();
                Shape::plain(match name {
                    // Both the iterator and the tuple around its items add a level.
                    Some("zip") => 2 + args.iter().map(item_depth).max().unwrap_or(0),
                    Some("enumerate") => 2 + args.first().map_or(0, item_depth),
                    Some("tuple" | "set" | "frozenset" | "iter" | "reversed") => {
                        1 + args.first().map_or(0, item_depth)
                    }
                    Some("dict") => args.first().map_or(1, |e| rank(e).max(1)),
                    Some("next") => args.first().map_or(0, |e| rank(e).saturating_sub(1)),
                    Some(
                        "len" | "int" | "float" | "str" | "repr" | "bool" | "bytes" | "sum"
                        | "print",
                    ) => 0,
                    Some(name) if self.fixed.contains(name) => 0,
                    _ => unknown_call_depth(
                        node.func
                            .as_attribute_expr()
                            .map_or(0, |attr| rank(&attr.value)),
                    ),
                })
            }
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
                            (Contents::List(_), Contents::List(_))
                        ) {
                            Shape::list(depth, old.list_layers().min(field.list_layers()))
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
            Expr::Call(_) => self.shape(value, locals).depth,
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
                    match (&old.contents, &shape.contents) {
                        (Contents::List(_), Contents::List(_)) => {
                            Shape::list(depth, old.list_layers().min(shape.list_layers()))
                        }
                        (Contents::Dict { key_depth: old }, Contents::Dict { key_depth: new }) => {
                            Shape {
                                depth,
                                contents: Contents::Dict {
                                    key_depth: (*old).max(*new),
                                },
                            }
                        }
                        _ => Shape::plain(depth),
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
        self.read_shapes.clear();
        let writes = crate::narrowing::expression_assignments(expr);
        if writes.is_empty() {
            return;
        }
        // A later assignment expression can rebind a name whose previous
        // value was already captured in a tuple. Record each affected read at
        // evaluation time so positional projection sees the captured value.
        struct Evaluation<'s, 'a> {
            shapes: &'s mut Shapes<'a>,
            writes: HashSet<String>,
            may_skip: bool,
            optional_writes: Vec<(&'a str, Shape)>,
        }
        impl Evaluation<'_, '_> {
            fn optional(&mut self, visit: impl FnOnce(&mut Self)) {
                let previous = std::mem::replace(&mut self.may_skip, true);
                let start = self.optional_writes.len();
                visit(self);
                // A later read on this path saw the write, but a path that
                // skipped it must still see the joined binding after this point.
                for (name, _) in &self.optional_writes[start..] {
                    self.shapes.straight_line.remove(*name);
                }
                if !previous {
                    self.optional_writes.truncate(start);
                }
                self.may_skip = previous;
            }
        }
        impl<'a> Visitor<'a> for Evaluation<'_, 'a> {
            fn visit_expr(&mut self, expr: &'a Expr) {
                match expr {
                    Expr::Name(node) if self.writes.contains(node.id.as_str()) => {
                        let value = self.shapes.lookup(node.id.as_str());
                        self.shapes.read_shapes.insert(node.range.start(), value);
                    }
                    Expr::Named(node) => {
                        self.visit_expr(&node.value);
                        let value = self.shapes.shape(&node.value, &[]);
                        let outer = self.shapes.conditional;
                        self.shapes.conditional |= self.may_skip;
                        if self.may_skip
                            && let Expr::Name(name) = &*node.target
                        {
                            let name = name.id.as_str();
                            self.optional_writes.push((name, self.shapes.lookup(name)));
                        }
                        self.shapes.bind(&node.target, value, true);
                        self.shapes.conditional = outer;
                    }
                    Expr::BoolOp(node) => {
                        if let Some((first, rest)) = node.values.split_first() {
                            self.visit_expr(first);
                            self.optional(|v| rest.iter().for_each(|item| v.visit_expr(item)));
                        }
                    }
                    Expr::If(node) => {
                        self.visit_expr(&node.test);
                        self.optional(|v| {
                            let start = v.optional_writes.len();
                            v.visit_expr(&node.body);
                            // Both arms start with the bindings after the test.
                            for (name, shape) in v.optional_writes[start..].iter().rev() {
                                v.shapes.straight_line.insert(name, shape.clone());
                            }
                            v.visit_expr(&node.orelse);
                        });
                    }
                    Expr::Compare(node) if node.operands.len() > 2 => {
                        let (first, rest) = node.operands.split_at(2);
                        first.iter().for_each(|item| self.visit_expr(item));
                        self.optional(|v| rest.iter().for_each(|item| v.visit_expr(item)));
                    }
                    Expr::ListComp(_)
                    | Expr::SetComp(_)
                    | Expr::DictComp(_)
                    | Expr::Generator(_) => {
                        self.optional(|v| walk_expr(v, expr));
                    }
                    Expr::Lambda(_) => {}
                    _ => walk_expr(self, expr),
                }
            }
        }
        Evaluation {
            shapes: self,
            writes,
            may_skip: false,
            optional_writes: Vec::new(),
        }
        .visit_expr(expr);
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
    fn optional_walrus_writes_are_definite_on_the_path_that_reaches_them() {
        for expression in [
            "(((t := None), (t,))[1] if flag else (None,))",
            "((None,) if flag else ((t := None), (t,))[1])",
            "[((t := None), (t,))[1] for item in [0]][0]",
            "[(t,) for item in [0] if (t := None) is None][0]",
            "(((t := None), (((t := x), (None,))[1] if branch else (t,)))[1] if flag else (None,))",
        ] {
            let step = format!("    t = x if flag else None\n    x = {expression}\n");
            let source = format!(
                "def f(flag: bool, branch: bool) -> int:\n    x = (None,)\n{}    return len(x)\n",
                step.repeat(MAX_DEPTH + 1)
            );
            crate::check(&source).unwrap();
        }
    }

    #[test]
    fn optional_walrus_writes_do_not_leak_into_the_other_arm() {
        let step =
            "    t = x if flag else None\n    x = (((t := None), (t,))[1] if reset else (t,))\n";
        let source = format!(
            "def f(flag: bool, reset: bool) -> int:\n    x = (0,)\n{}",
            step.repeat(MAX_DEPTH + 1)
        );
        assert!(limited(&source));
    }

    #[test]
    fn repr_returns_a_scalar_for_nested_arguments() {
        for wrap in [
            "list(zip(repr(x)[:1]))",
            "list(enumerate(repr(x)[:1]))",
            "(repr(x),)",
        ] {
            let body = format!("x = [1]\n{}", format!("x = {wrap}\n").repeat(300));
            assert!(!limited(&body), "{wrap}");
        }
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
    fn chained_pop_calls_visit_each_unknown_receiver_once() {
        let levels = 36;
        let annotation = format!("{}int{}", "list[".repeat(levels), "]".repeat(levels));
        let source = format!(
            "def f(x: {annotation}) -> int:\n{}    value = x{}\n    return value\n",
            "    pass\n".repeat(115),
            ".pop()".repeat(levels),
        );
        crate::check(&source).unwrap();
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
    fn conditional_dictionary_bindings_keep_key_depth() {
        let mut source = String::from("def f(flag: bool) -> int:\n    x = [(1,)]\n");
        for _ in 0..=MAX_DEPTH {
            source.push_str(
                "    d = {0: x}\n    if flag:\n        d = {0: x}\n    else:\n        d = {1: x}\n    x = list(zip(d))\n",
            );
        }
        source.push_str("    return len(x)\n");
        crate::check(&source).unwrap();
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

    #[test]
    fn tuple_fields_capture_names_before_their_assignment_expressions() {
        for (fields, index, too_deep) in [
            ("$name, ($name := None), $previous", 0, true),
            ("($name := None), $name, $previous", 1, false),
            (
                "flag or (($name := None) is None), $name, $previous",
                1,
                true,
            ),
            (
                "False if flag else (($name := None) is None), $name, $previous",
                1,
                true,
            ),
        ] {
            let mut source = String::from("def f(flag: bool) -> None:\n    v0 = (1,)\n");
            for i in 1..=MAX_DEPTH + 1 {
                let previous = format!("v{}", i - 1);
                let name = format!("t{i}");
                let fields = fields
                    .replace("$name", &name)
                    .replace("$previous", &previous);
                source.push_str(&format!(
                    "    {name} = {previous} if flag else None\n    pair{i} = ({fields})\n    v{i} = (pair{i}[{index}],)\n"
                ));
            }
            let result = crate::check(&source);
            if too_deep {
                assert!(result.unwrap_err().contains("inferred container nesting"));
            } else {
                result.unwrap();
            }
        }
    }

    #[test]
    fn list_copies_and_projections_preserve_nested_pop_shapes() {
        for expr in [
            "[[v]].pop().pop()",
            "[[v]][0].pop()",
            "list([v]).pop()",
            "sorted([v]).pop()",
        ] {
            let mut source = String::from("def f() -> int:\n    v = 1\n");
            for _ in 0..MAX_DEPTH + 1 {
                source.push_str(&format!("    v = {expr}\n"));
            }
            source.push_str("    return v\n");
            crate::check(&source).unwrap();
        }
    }
}
