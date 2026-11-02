//! Instance attributes first assigned by a native TestCase fixture.
//! Constructing a case leaves these slots absent. Completing each assignment
//! publishes its value, independently of whether the rest of setUp succeeds.

use std::collections::BTreeMap;

use ruff_python_ast::visitor::{Visitor, walk_stmt};

use super::*;

#[derive(Clone, Copy)]
pub(super) struct Field {
    ready: usize,
    value: usize,
    ty: Type,
    pub(super) shared: Option<class_data::Shadow>,
}

pub(super) type Slots = Vec<HashMap<String, Field>>;

struct Writes<'a> {
    receiver: &'a str,
    assignments: Vec<(&'a str, &'a ast::Expr)>,
}

impl<'a> Visitor<'a> for Writes<'a> {
    fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
        if matches!(stmt, ast::Stmt::ClassDef(_) | ast::Stmt::FunctionDef(_)) {
            return;
        }
        if let ast::Stmt::Assign(assign) = stmt {
            for target in &assign.targets {
                if let ast::Expr::Attribute(attr) = target
                    && attr
                        .value
                        .as_name_expr()
                        .is_some_and(|n| n.id == self.receiver)
                {
                    self.assignments.push((attr.attr.as_str(), &assign.value));
                }
            }
        }
        walk_stmt(self, stmt);
    }
}

/// Resolve only results which have a closed Python type. The ordinary expression
/// checker still checks the call, its arguments and every subsequent assignment.
fn value_type(
    graph: &SourceGraph,
    module: usize,
    classes: &Classes,
    types: &Types,
    value: &ast::Expr,
) -> Option<Type> {
    Some(match value {
        ast::Expr::StringLiteral(_) => Type::Str,
        ast::Expr::BytesLiteral(_) => Type::Bytes,
        ast::Expr::BooleanLiteral(_) => Type::Bool,
        ast::Expr::NoneLiteral(_) => Type::None,
        ast::Expr::NumberLiteral(n) => match n.value {
            ast::Number::Int(_) => Type::Int,
            ast::Number::Float(_) => Type::Float,
            _ => return None,
        },
        ast::Expr::Call(c)
            if c.func.as_name_expr().is_some_and(|n| n.id == "dict")
                && c.arguments.keywords.is_empty()
                && let [env @ ast::Expr::Attribute(mapping)] = c.arguments.args.as_slice()
                && graph.session.is_known_class(
                    module,
                    &c.func,
                    ty_python_semantic::types::KnownClass::Dict,
                )
                && graph.session.is_stdlib_module(module, &mapping.value, "os") =>
        {
            let item = match graph.resolve(module, env) {
                Some(Binding::Intrinsic(Intrinsic::Environ)) => Type::Str,
                Some(Binding::Intrinsic(Intrinsic::EnvironBytes)) => Type::Bytes,
                _ => return None,
            };
            types.dict(item, item)
        }
        ast::Expr::Call(c) => {
            let Binding::Definition(def) = graph.resolve(module, &c.func)? else {
                return None;
            };
            if let Some(&id) = classes.ids.get(&def) {
                Type::Class(id)
            } else {
                let ast::Stmt::FunctionDef(f) =
                    &graph.modules[def.module].parsed.syntax().body[def.statement]
                else {
                    return None;
                };
                if !f.decorator_list.is_empty() || f.is_async {
                    return None;
                }
                annotation_in_class(
                    f.returns.as_deref()?,
                    graph,
                    def.module,
                    classes,
                    types,
                    None,
                )
                .ok()?
            }
        }
        _ => return None,
    })
}

/// Reserve the same private offsets on related cases: a base helper can read a
/// field first written by a subclass. Existing fields and descriptors keep their
/// ordinary access path. These slots participate in the usual ownership checks.
pub(super) fn slots(
    graph: &SourceGraph,
    types: &Types,
    classes: &mut Classes,
    fields: &mut [Vec<Type>],
) -> Result<Slots, String> {
    let mut slots = vec![HashMap::new(); fields.len()];
    let Some(case) = classes.definitions.iter().position(|c| {
        let m = &graph.modules[c.definition.module];
        m.bundled && m.name == "unittest" && c.name == "TestCase"
    }) else {
        return Ok(slots);
    };
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (id, c) in classes.definitions.iter().enumerate() {
        if let Some(pos) = c.bases.iter().position(|&base| base == case) {
            groups
                .entry(if pos == 0 { id } else { c.bases[pos - 1] })
                .or_default()
                .push(id);
        }
    }
    let owners: Vec<_> = classes.definitions.iter().map(|c| c.name.clone()).collect();
    for group in groups.values() {
        let mut private = HashSet::new();
        let mut names: BTreeMap<String, Option<Type>> = BTreeMap::new();
        for &id in group {
            let c = &classes.definitions[id];
            let Some(setup) = c.node.body.iter().find_map(|s| match s {
                ast::Stmt::FunctionDef(f)
                    if f.name.as_str() == "setUp" && f.decorator_list.is_empty() && !f.is_async =>
                {
                    Some(f)
                }
                _ => None,
            }) else {
                continue;
            };
            let Some(arg) = setup
                .parameters
                .posonlyargs
                .iter()
                .chain(&setup.parameters.args)
                .next()
            else {
                continue;
            };
            let mut writes = Writes {
                receiver: arg.parameter.name.as_str(),
                assignments: vec![],
            };
            for stmt in &setup.body {
                writes.visit_stmt(stmt);
            }
            for (name, value) in writes.assignments {
                let Some(ty) = value_type(graph, c.definition.module, classes, types, value) else {
                    continue;
                };
                let name = if let Some(key) = crate::classes::private_field(&c.name, name) {
                    // Class-data publication scans the original source. Until private
                    // declarations share that normalization, never invent a second slot.
                    if group.iter().any(|&member| {
                        let def = &classes.definitions[member];
                        [name, key.as_str()].into_iter().any(|member| {
                            def.static_bindings.contains_key(member)
                                || def.fields.iter().any(|f| f.name == member)
                                || def.methods.iter().any(|m| m.name.as_str() == member)
                        })
                    }) {
                        return Err(graph.modules[c.definition.module].error(
                            value,
                            "private fixture field overlaps a declared class member",
                        ));
                    }
                    private.insert(key.clone());
                    key
                } else if c.fields.iter().any(|f| f.name == name)
                    || c.methods.iter().any(|m| m.name.as_str() == name)
                {
                    continue;
                } else {
                    name.to_owned()
                };
                names
                    .entry(name)
                    .and_modify(|prior| {
                        if let Some(p) = *prior
                            && p != ty
                        {
                            *prior = if p == Type::None {
                                Some(types.optional(ty))
                            } else if ty == Type::None {
                                Some(types.optional(p))
                            } else if types.payload(p) == Some(ty) {
                                Some(p)
                            } else {
                                None
                            };
                        }
                    })
                    .or_insert(Some(ty));
            }
        }
        let mut end = group.iter().map(|&id| fields[id].len()).max().unwrap();
        for (name, ty) in names {
            let Some(ty) = ty else { continue };
            if callables::contains_callable(ty, types) {
                continue;
            }
            // A declared field or descriptor has its own storage and dispatch.
            // Do not invent a second interpretation on a base-typed receiver.
            if group.iter().any(|&id| {
                let c = &classes.definitions[id];
                c.fields.iter().any(|f| f.name == name)
                    || c.methods.iter().any(|m| m.name.as_str() == name)
            }) {
                continue;
            }
            // A setup write and an ordinary instance write must publish the same
            // value. A base helper can also read a sibling without a default.
            let mut ty = ty;
            let mut overlaps = false;
            for &id in group {
                let Some(crate::classes::ClassValue::Runtime { owner, ordinal }) =
                    classes.definitions[id].static_bindings.get(&name)
                else {
                    continue;
                };
                overlaps = true;
                let ast::Stmt::Assign(assign) =
                    &classes.definitions[classes.ids[owner]].node.body[*ordinal]
                else {
                    unreachable!()
                };
                if let Some(default) =
                    value_type(graph, owner.module, classes, types, &assign.value)
                {
                    if ty == Type::None || types.payload(default) == Some(ty) {
                        ty = types.optional(default);
                    } else if default == Type::None {
                        ty = types.optional(ty);
                    }
                }
            }
            let shared = overlaps.then(|| class_data::Shadow::new(end, end + 1, ty, types));
            let field = Field {
                ready: end,
                value: end + 1,
                ty,
                shared,
            };
            for &id in group {
                fields[id].resize(end, Type::Bool);
                fields[id].extend([Type::Bool, if overlaps { types.optional(ty) } else { ty }]);
                slots[id].insert(name.to_owned(), field);
            }
            end += 2;
        }
        if !private.is_empty() {
            // Only source attributes mangle. An inherited method keeps its declaring
            // class, while literal getattr/attrgetter names keep their exact spelling.
            for &id in group {
                let c = &mut classes.definitions[id];
                for (method, origin) in c.methods.iter_mut().zip(&c.method_origins) {
                    crate::classes::mangle_fields(&owners[classes.ids[origin]], method, |key| {
                        private.contains(key)
                    });
                }
            }
        }
    }
    Ok(slots)
}

impl Checker<'_> {
    pub(super) fn setup_field(&self, ty: Type, name: &str) -> Option<Field> {
        let Type::Class(id) = ty else { return None };
        self.classes.setup_fields.get(id)?.get(name).copied()
    }

    pub(super) fn read_setup_field(
        &mut self,
        receiver: Expr,
        attribute: &ast::ExprAttribute,
        field: Field,
    ) -> Result<Expr, String> {
        let Type::Class(class) = receiver.ty else {
            unreachable!()
        };
        let name = attribute.attr.as_str();
        let object = self.hidden(receiver.ty);
        let result = self.hidden(field.ty);
        let local = self.local(object);
        let load = |index, ty| Expr {
            ty,
            kind: ExprKind::Field(Box::new(local.clone()), index),
        };
        let absent = |checker: &mut Self, id: usize| -> Result<Statement, String> {
            let definition = &checker.classes.declarations.definitions[id];
            if definition.static_bindings.contains_key(name) {
                let default = checker.class_attribute(attribute, id, None, None)?;
                return Ok(Statement::Assign(
                    result,
                    checker.coerce(default, field.ty, attribute)?,
                ));
            }
            Ok(Statement::Raise(Some(Expr {
                ty: Type::Exception,
                kind: ExprKind::Exception {
                    kind: exception_kind("AttributeError").unwrap(),
                    message: Some(Box::new(unpacking::string(format!(
                        "'{}' object has no attribute '{name}'",
                        definition.name
                    )))),
                },
            })))
        };
        let mut missing = absent(self, class)?;
        for child in self.classes.concrete_layouts(class) {
            if child != class {
                missing = Statement::If(
                    Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Builtin(Builtin::ExactRecord(child), vec![local.clone()]),
                    },
                    vec![absent(self, child)?],
                    vec![missing],
                );
            }
        }
        Ok(Expr {
            ty: field.ty,
            kind: ExprKind::Scope {
                body: vec![
                    Statement::Assign(object, receiver),
                    Statement::If(
                        load(field.ready, Type::Bool),
                        vec![Statement::Assign(
                            result,
                            field
                                .shared
                                .map_or_else(|| load(field.value, field.ty), |s| s.payload(&local)),
                        )],
                        vec![missing],
                    ),
                ],
                value: Box::new(self.local(result)),
                clear: vec![object, result],
            },
        })
    }

    pub(super) fn store_setup_field(
        &mut self,
        receiver: Expr,
        value: Expr,
        at: &ast::ExprAttribute,
        field: Field,
    ) -> Result<Statement, String> {
        let value = self.coerce(value, field.ty, at)?;
        let stored = if field.shared.is_some() {
            self.types.optional(field.ty)
        } else {
            field.ty
        };
        let value = self.coerce(value, stored, at)?;
        let (rhs, object) = (self.hidden(stored), self.hidden(receiver.ty));
        Ok(Statement::Scope {
            body: vec![
                Statement::Assign(rhs, value),
                Statement::Assign(object, receiver),
                Statement::SetField(self.local(object), field.value, self.local(rhs)),
                Statement::SetField(self.local(object), field.ready, Expr::boolean(true)),
            ],
            clear: vec![rhs, object],
        })
    }
}
