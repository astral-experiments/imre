//! Resolve one concrete layout lineage and infer straight-line constructor fields.

use super::*;
use ruff_text_size::Ranged;

pub(super) fn resolve<'syntax>(
    graph: &SourceGraph,
    nodes: &[(DefinitionId, Cow<'syntax, ast::StmtClassDef>)],
    ids: &mut HashMap<DefinitionId, usize>,
    exceptions: &exceptions::Declarations,
    first_layout: Option<&mut Option<Diagnostic>>,
) -> Result<(Vec<Class<'syntax>>, HashSet<DefinitionId>), Diagnostic> {
    struct Layouts<'syntax> {
        fixed: Vec<Option<Class<'syntax>>>,
        registered: HashSet<DefinitionId>,
    }
    fn build<'syntax>(
        id: usize,
        graph: &SourceGraph,
        nodes: &[(DefinitionId, Cow<'syntax, ast::StmtClassDef>)],
        ids: &HashMap<DefinitionId, usize>,
        exceptions: &exceptions::Declarations,
        layouts: &mut Layouts<'syntax>,
        path: &mut Vec<usize>,
    ) -> Result<(), Diagnostic> {
        if layouts.fixed[id].is_some() || layouts.registered.contains(&nodes[id].0) {
            return Ok(());
        }
        let (definition, ref source) = nodes[id];
        let node = source.as_ref();
        let module = &graph.modules[definition.module];
        if path.contains(&id) || path.len() >= 64 {
            return Err(module.diagnostic(node, "cyclic or too deeply nested class lineage"));
        }
        path.push(id);
        let mut base: Option<usize> = None;
        let mut mixin = None;
        let mut leading_mixin = false;
        if let Some(args) = &node.arguments {
            for argument in &args.args {
                if graph.is_protocol_base(definition.module, argument) {
                    graph.needs_object_runtime.set(true);
                    return Err(
                        module.diagnostic(argument, "Protocol bases require object lowering")
                    );
                }
                if mapping_base(graph, definition.module, argument)
                    || (args.args.len() == 1
                        && graph
                            .session
                            .builtin_object_base(definition.module, argument))
                {
                    continue;
                }
                let origin = match argument {
                    ast::Expr::Subscript(subscript)
                        if matches!(graph.resolve(definition.module, &subscript.value),
                        Some(Binding::Definition(id)) if graph.is_abstract_context_manager(id)) =>
                    {
                        &*subscript.value
                    }
                    other => other,
                };
                match graph.resolve(definition.module, origin) {
                    Some(Binding::Intrinsic(Intrinsic::PICKLER)) => {
                        graph.needs_object_runtime.set(true);
                        return Err(
                            module.diagnostic(argument, "pickle.Pickler requires object lowering")
                        );
                    }
                    Some(Binding::Intrinsic(Intrinsic::ObjectRuntimeExport(
                        "contextlib.ContextDecorator",
                    ))) => {
                        graph.needs_object_runtime.set(true);
                        return Err(module.diagnostic(
                            argument,
                            "ContextDecorator subclasses require object lowering",
                        ));
                    }
                    Some(Binding::Intrinsic(Intrinsic::ObjectRuntimeExport(
                        "codecs.IncrementalDecoder" | "codecs.CodecInfo",
                    ))) => {
                        graph.needs_object_runtime.set(true);
                        return Err(module
                            .diagnostic(argument, "codec base class requires object lowering"));
                    }
                    Some(Binding::Intrinsic(
                        Intrinsic::IO_BASE
                        | Intrinsic::RAW_IO_BASE
                        | Intrinsic::BUFFERED_IO_BASE
                        | Intrinsic::TEXT_IO_BASE
                        | Intrinsic::ObjectRuntimeExport(
                            "io.BufferedReader"
                            | "_io.BufferedReader"
                            | "io.BufferedRandom"
                            | "_io.BufferedRandom"
                            | "io.BufferedWriter"
                            | "_io.BufferedWriter"
                            | "io.BufferedRWPair"
                            | "_io.BufferedRWPair"
                            | "_io._IOBase"
                            | "_io._RawIOBase"
                            | "_io._BufferedIOBase"
                            | "_io._TextIOBase",
                        ),
                    )) => {
                        graph.needs_object_runtime.set(true);
                        return Err(module
                            .diagnostic(argument, "I/O base subclasses require object lowering"));
                    }
                    Some(Binding::Intrinsic(base)) if base.is_subclassable_calendar() => {
                        graph.needs_object_runtime.set(true);
                        return Err(module
                            .diagnostic(argument, "calendar subclasses require object lowering"));
                    }
                    Some(Binding::Intrinsic(Intrinsic::StrEnum | Intrinsic::StandardEnum))
                        if args.args.len() == 1 && args.keywords.is_empty() => {}
                    Some(Binding::Intrinsic(Intrinsic::Abc)) => {}
                    Some(Binding::Definition(source)) if ids.contains_key(&source) => {
                        if source.module != definition.module
                            && let Some(name) = graph.modules[source.module].python_subclasses()
                        {
                            graph.needs_object_runtime.set(true);
                            return Err(module
                                .diagnostic(argument, &format!("{name} require object lowering")));
                        }
                        if source.module == definition.module
                            && source.statement >= definition.statement
                        {
                            return Err(
                                module.diagnostic(argument, "a base class must already be defined")
                            );
                        }
                        let next = ids[&source];
                        build(next, graph, nodes, ids, exceptions, layouts, path)?;
                        if layouts.registered.contains(&source) {
                            layouts.registered.insert(definition);
                            path.pop();
                            return Ok(());
                        }
                        if let Some(first) = base {
                            let parent: &Class = layouts.fixed[first].as_ref().unwrap();
                            if parent.bases.contains(&next) {
                                continue;
                            }
                            if args.args.len() == 2 && first != next {
                                let second = layouts.fixed[next].as_ref().unwrap();
                                if method_only_mixin(graph, second) {
                                    mixin = Some(next);
                                    continue;
                                }
                                if !second.bases.contains(&first)
                                    && independent_leading_mixin(graph, parent, second)
                                {
                                    base = Some(next);
                                    mixin = Some(first);
                                    leading_mixin = true;
                                    continue;
                                }
                            }
                            return Err(module.diagnostic(
                                argument,
                                "multiple independent base classes are unsupported",
                            ));
                        } else {
                            base = Some(next);
                        }
                    }
                    _ => {
                        let origin = match argument {
                            ast::Expr::Subscript(subscript) => &*subscript.value,
                            other => other,
                        };
                        if graph
                            .session
                            .is_builtin_sequence_class(definition.module, origin)
                            || graph
                                .session
                                .is_collections_storage_base(definition.module, argument)
                            || graph.session.is_known_class(
                                definition.module,
                                argument,
                                ty_python_semantic::types::KnownClass::Super,
                            )
                        {
                            let error = module
                                .diagnostic(argument, "native base class requires object lowering");
                            return Err(error.classified(Kind::ScopedObject));
                        }
                        return Err(module
                            .diagnostic(argument, "the base must be a known fixed-layout class"));
                    }
                }
            }
        }
        let mut decorators = DeferredMethodDecorators::default();
        let class = collect_class(
            graph,
            module,
            definition,
            source,
            [base, mixin].map(|parent| parent.map(|id| (id, layouts.fixed[id].as_ref().unwrap()))),
            exceptions,
            &mut decorators,
        );
        if !decorators.has_runtime_method
            && let Some(error) = decorators.unsupported
        {
            return Err(error);
        }
        let Some(mut class) = class? else {
            layouts.registered.insert(definition);
            path.pop();
            return Ok(());
        };
        if leading_mixin {
            // The independent root has no ancestors. It precedes the entire storage lineage.
            class.bases.rotate_right(1);
        }
        if exceptions.layouts.contains(&definition) {
            let index = (exceptions.ids[&definition] - exceptions::FIRST_USER_EXCEPTION) as usize;
            let mro = &exceptions.named[index].ancestors;
            if let Some(field) = class
                .fields
                .iter()
                .find(|field| exceptions::inherited_descriptor(&field.name, mro))
            {
                return Err(module.diagnostic(
                    node,
                    &format!(
                        "a record field cannot store the inherited exception descriptor '{}'",
                        field.name
                    ),
                ));
            }
            if base.is_none() {
                exceptions::install_argument_slot(&mut class, module);
            }
        }
        layouts.fixed[id] = Some(class);
        path.pop();
        Ok(())
    }
    let mut layouts = Layouts {
        fixed: (0..nodes.len()).map(|_| None).collect(),
        registered: HashSet::new(),
    };
    let mut path = Vec::new();
    for id in 0..nodes.len() {
        if let Err(error) = build(id, graph, nodes, ids, exceptions, &mut layouts, &mut path) {
            if error.kind == Kind::ClassLayout
                && graph.class_layout_error.borrow().is_none()
                && let Some(first_layout) = first_layout
                // Exception identities and their descendants share the complete payload layouts.
                && exceptions.layouts.iter().all(|owner| {
                    layouts.registered.contains(owner)
                        || ids.get(owner).is_some_and(|&id| layouts.fixed[id].is_some())
                })
            {
                *first_layout = Some(error);
                break;
            }
            return Err(error);
        }
    }
    let mut fixed = layouts.fixed.into_iter().flatten().collect::<Vec<_>>();
    if fixed.len() != nodes.len() {
        ids.clear();
        ids.extend(
            fixed
                .iter()
                .enumerate()
                .map(|(id, class)| (class.definition, id)),
        );
        let native = |id: usize| ids[&nodes[id].0];
        for class in &mut fixed {
            class.storage_base = class.storage_base.map(native);
            for base in &mut class.bases {
                *base = native(*base);
            }
        }
    }
    Ok((fixed, layouts.registered))
}

/// A root mixin supplies ordinary methods without declaring receiver storage.
fn method_only_mixin(graph: &SourceGraph, class: &Class) -> bool {
    let node = &class.node;
    node.decorator_list.is_empty()
        && node.type_params.is_none()
        && node.arguments.as_ref().is_none_or(|args| {
            args.keywords.is_empty()
                && (args.args.is_empty()
                    || matches!(&*args.args, [base]
                if graph.session.builtin_object_base(class.definition.module, base)))
        })
        && node.body.iter().all(|stmt| match stmt {
            ast::Stmt::FunctionDef(f) => !f.name.starts_with("__") && f.decorator_list.is_empty(),
            ast::Stmt::Expr(expr) => matches!(&*expr.value, ast::Expr::StringLiteral(_)),
            ast::Stmt::Pass(_) => true,
            _ => false,
        })
}

/// The fixed layout can reuse disjoint methods, but cannot run a cooperative
/// mixin method against different next bases depending on the receiver.
fn independent_leading_mixin(graph: &SourceGraph, mixin: &Class, storage: &Class) -> bool {
    method_only_mixin(graph, mixin)
        && !ast::helpers::any_over_body(
            &mixin.node.body,
            |expr| matches!(expr, ast::Expr::Name(name) if name.id == "super"),
        )
        && mixin
            .methods
            .iter()
            .filter(|method| method.name.as_str() != "__init__")
            .all(|method| {
                let name = method.name.as_str();
                !storage
                    .methods
                    .iter()
                    .any(|other| other.name.as_str() == name)
                    && !storage.fields.iter().any(|field| field.name == name)
                    && !storage.static_bindings.contains_key(name)
                    && !storage.exception_aliases.contains_key(name)
            })
}

/// Return the member and the name whose binding must be the builtin `super`.
pub(crate) fn super_method(call: &ast::ExprCall) -> Option<(&ast::ExprAttribute, &ast::Expr)> {
    let method = call.func.as_attribute_expr()?;
    let proxy = method.value.as_call_expr()?;
    (proxy.arguments.is_empty() && proxy.func.as_name_expr()?.id == "super")
        .then_some((method, &proxy.func))
}

pub(crate) fn super_init(call: &ast::ExprCall) -> bool {
    super_method(call).is_some_and(|(method, _)| method.attr.as_str() == "__init__")
}

/// Resolve an explicit initializer on the lexical parent and the current receiver.
pub(crate) fn direct_base_init(
    graph: &SourceGraph,
    module: usize,
    parent: DefinitionId,
    initializer: &ast::StmtFunctionDef,
    call: &ast::ExprCall,
) -> bool {
    let Some(method) = call.func.as_attribute_expr() else {
        return false;
    };
    let Some(receiver) = initializer.parameters.iter_non_variadic_params().next() else {
        return false;
    };
    if method.attr.as_str() != "__init__"
        || !matches!(call.arguments.args.first(), Some(ast::Expr::Name(n)) if n.id.as_str() == receiver.name().as_str())
        || graph.resolve(module, &method.value) != Some(Binding::Definition(parent))
    {
        return false;
    }
    let mut root = &*method.value;
    while let ast::Expr::Attribute(attr) = root {
        root = &attr.value;
    }
    let Some(root) = root.as_name_expr() else {
        return false;
    };
    // A parameter or local variable with the same spelling takes precedence over the module binding.
    !initializer
        .parameters
        .iter()
        .any(|p| p.name() == root.id.as_str())
        && !ast::helpers::any_over_body(
            &initializer.body,
            |expr| matches!(expr, ast::Expr::Name(n) if n.id == root.id && n.ctx != ast::ExprContext::Load),
        )
}

/// Infer scalar field storage without changing how the constructor evaluates its value.
fn scalar_field_annotation(
    graph: &SourceGraph,
    module: usize,
    value: &ast::Expr,
) -> Option<ast::Expr> {
    let name = match value {
        ast::Expr::BooleanLiteral(_) => "bool",
        ast::Expr::StringLiteral(_) => "str",
        ast::Expr::Call(call)
            if let ast::Expr::Attribute(method) = &*call.func
                && method.attr.as_str() == "format"
                && matches!(&*method.value, ast::Expr::StringLiteral(_)) =>
        {
            "str"
        }
        _ if graph.session.has_string_layout(module, value) => "str",
        _ => return None,
    };
    let annotation = ast::Expr::Name(ast::ExprName {
        node_index: ast::AtomicNodeIndex::NONE,
        range: value.range(),
        id: name.into(),
        ctx: ast::ExprContext::Load,
    });
    // An inferred builtin must not resolve to a user-defined class or type alias.
    graph
        .resolve_type(module, &annotation)
        .is_none()
        .then_some(annotation)
}

/// Fold a constructor attribute annotation into the fixed layout. Python does not
/// evaluate this annotation, so the checked method subsequently performs a normal store.
pub(super) fn annotated_fields(
    graph: &SourceGraph,
    module: usize,
    init: &mut ast::StmtFunctionDef,
    fields: &mut Vec<Field>,
) {
    let Some(receiver) = init.parameters.iter_non_variadic_params().next() else {
        return;
    };
    let params = init
        .parameters
        .iter_non_variadic_params()
        .map(|p| (p.name().as_str(), p.annotation()))
        .collect::<HashMap<_, _>>();
    let locals = crate::narrowing::assigned_names(&init.body);
    for statement in &mut init.body {
        let (target, value, annotation) = match &*statement {
            ast::Stmt::AnnAssign(a)
                if let (ast::Expr::Attribute(target), Some(value)) = (&*a.target, &a.value) =>
            {
                (target, &**value, Some(&*a.annotation))
            }
            ast::Stmt::Assign(a) if let [ast::Expr::Attribute(target)] = a.targets.as_slice() => {
                (target, &*a.value, None)
            }
            _ => continue,
        };
        if !matches!(&*target.value, ast::Expr::Name(n) if n.id.as_str() == receiver.name().as_str())
        {
            continue;
        }
        let proxy_arg = if let ast::Expr::Call(call) = value
            && graph.resolve(module, &call.func) == Some(Binding::Intrinsic(Intrinsic::WeakProxy))
        {
            let mut root = &*call.func;
            while let ast::Expr::Attribute(attr) = root {
                root = &attr.value;
            }
            root.as_name_expr()
                .filter(|n| !params.contains_key(n.id.as_str()) && !locals.contains(n.id.as_str()))
                .and_then(|_| call.arguments.args.first())
        } else {
            None
        };
        if let Some(field) = fields.iter_mut().find(|f| f.name == target.attr.as_str()) {
            field.weak_proxy |= proxy_arg.is_some();
        } else if let Some(annotation) = annotation.or_else(|| {
            let name = proxy_arg?.as_name_expr()?.id.as_str();
            params.get(name).copied().flatten()
        }) {
            fields.push(Field {
                module,
                name: target.attr.to_string(),
                annotation: annotation.clone(),
                weak_proxy: proxy_arg.is_some(),
                default: None,
                kw_only: false,
                repr: true,
                compare: true,
            });
        }
        if let ast::Stmt::AnnAssign(assign) = statement
            && let Some(value) = &assign.value
        {
            *statement = ast::Stmt::Assign(ast::StmtAssign {
                node_index: ast::AtomicNodeIndex::NONE,
                range: assign.range,
                targets: vec![(*assign.target).clone()],
                value: value.clone(),
            });
        }
    }
}

/// Returns whether an unannotated argument supplies an instance field.
pub(super) fn infer_fields(
    graph: &SourceGraph,
    module: usize,
    init: &ast::StmtFunctionDef,
    fields: &mut Vec<Field>,
) -> bool {
    let mut open_parameter_field = false;
    let params = init
        .parameters
        .iter_non_variadic_params()
        .map(|p| (p.name().as_str(), p.annotation()))
        .collect::<HashMap<_, _>>();
    for stmt in &init.body {
        let ast::Stmt::Assign(assign) = stmt else {
            continue;
        };
        let [ast::Expr::Attribute(target)] = assign.targets.as_slice() else {
            continue;
        };
        if !matches!(&*target.value, ast::Expr::Name(name) if name.id == "self")
            || fields
                .iter()
                .any(|field| field.name == target.attr.as_str())
        {
            continue;
        }
        let ann = match &*assign.value {
            ast::Expr::Name(name) if let Some(annotation) = params.get(name.id.as_str()) => {
                open_parameter_field |= annotation.is_none();
                annotation.cloned()
            }
            value if let Some(annotation) = scalar_field_annotation(graph, module, value) => {
                Some(annotation)
            }
            ast::Expr::Call(call)
                if matches!(&*call.func, ast::Expr::Name(name) if name.id == "list")
                    && graph.resolve(module, &call.func).is_none()
                    && call.arguments.keywords.is_empty() =>
            {
                let [ast::Expr::Name(input)] = call.arguments.args.as_ref() else {
                    continue;
                };
                let Some(ast::Expr::Subscript(annotated)) =
                    params.get(input.id.as_str()).copied().flatten()
                else {
                    continue;
                };
                let mut list = annotated.clone();
                list.value.clone_from(&call.func);
                Some(ast::Expr::Subscript(list))
            }
            ast::Expr::ListComp(comp) => {
                let [generator] = comp.generators.as_slice() else {
                    continue;
                };
                let (ast::Expr::Attribute(input), ast::Expr::Name(item)) =
                    (&generator.iter, &generator.target)
                else {
                    continue;
                };
                if !matches!(&*input.value, ast::Expr::Name(name) if name.id == "self") {
                    continue;
                }
                let Some(field) = fields
                    .iter()
                    .find(|field| field.name == input.attr.as_str())
                else {
                    continue;
                };
                let ast::Expr::Subscript(list) = &field.annotation else {
                    continue;
                };
                let preserving = matches!(&*comp.elt, ast::Expr::Name(name) if name.id == item.id)
                    || matches!(&*comp.elt, ast::Expr::Subscript(slice) if matches!(&*slice.slice, ast::Expr::Slice(_))
                        && matches!(&*slice.value, ast::Expr::Name(name) if name.id == item.id)
                        && matches!(&*list.slice, ast::Expr::Name(ty) if matches!(ty.id.as_str(), "str" | "bytes")));
                preserving.then(|| field.annotation.clone())
            }
            _ => None,
        };
        if let Some(annotation) = ann {
            fields.push(Field {
                module,
                name: target.attr.to_string(),
                annotation,
                weak_proxy: false,
                default: None,
                kw_only: false,
                repr: true,
                compare: true,
            });
        }
    }
    open_parameter_field
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_fields_keep_native_and_python_bases_distinct() {
        let source = r#"class Open:
    def __init__(self, value):
        self.value = value

class Fixed:
    value: int
    def __init__(self, value: int):
        self.value = value

class OpenChild(Open):
    pass

class FixedChild(Fixed):
    pass

assert issubclass(OpenChild, Open)
assert not issubclass(Open, OpenChild)
"#;
        crate::compile(source, &crate::CompileOptions::default()).unwrap();
        let graph = SourceGraph::single(source).unwrap();
        let classes = Classes::collect(&graph).unwrap();
        let id = |statement| DefinitionId {
            module: 0,
            statement,
        };
        assert_eq!(classes.registered_classes, HashSet::from([id(0), id(2)]));
        assert_eq!(classes.ids[&id(1)], 0);
        let child = &classes.definitions[classes.ids[&id(3)]];
        assert_eq!(child.storage_base, Some(0));
        assert_eq!(child.bases, [0]);
    }

    #[test]
    fn property_uses_its_module_functions_or_its_instance_methods() {
        let graph = SourceGraph::single(
            r#"def read(owner): return owner.stored
def write(owner, value): owner.stored = value

class Captured:
    def __init__(self, value: int): self.stored = value
    item = property(read, write)

class WriteOnly:
    def __init__(self, value: int): self.stored = value
    def set(self, value: int): self.stored = value
    item = property(None, set)

class Fixed:
    def __init__(self, value: int): self.stored = value
    def get(self) -> int: return self.stored
    item = property(get)

class Child(Fixed):
    def get(self) -> int: return 23
"#,
        )
        .unwrap();
        let classes = Classes::collect(&graph).unwrap();
        let id = |statement| DefinitionId {
            module: graph.entry,
            statement,
        };
        assert_eq!(classes.registered_classes, HashSet::from([id(2), id(3)]));
        let fixed = &classes.definitions[classes.ids[&id(4)]];
        let child = &classes.definitions[classes.ids[&id(5)]];
        let method = |class: &super::Class, name: &str| {
            class
                .methods
                .iter()
                .position(|method| method.name.as_str() == name)
                .unwrap()
        };
        let get = &fixed.methods[method(fixed, "get")].function;
        let property = &fixed.methods[method(fixed, "item")].function;
        let inherited = &child.methods[method(child, "item")].function;
        assert!(std::sync::Arc::ptr_eq(get, property));
        assert!(std::sync::Arc::ptr_eq(get, inherited));
        assert!(!std::sync::Arc::ptr_eq(
            get,
            &child.methods[method(child, "get")].function
        ));
    }

    #[test]
    fn async_iterator_registers_only_its_class_and_subclasses() {
        let source = r#"from typing import AsyncIterator

class Stream:
    async def __aiter__(self) -> AsyncIterator[bytes]:
        raise NotImplementedError
        yield b""
    async def aclose(self) -> None:
        pass

class Scalar:
    def add(self, value: int) -> int:
        return value + 1

class Derived(Stream):
    async def __aiter__(self) -> AsyncIterator[bytes]:
        yield b"child"
"#;
        let graph = SourceGraph::single(source).unwrap();
        let classes = Classes::collect(&graph).unwrap();
        let id = |statement| DefinitionId {
            module: graph.entry,
            statement,
        };
        assert_eq!(classes.registered_classes, HashSet::from([id(1), id(3)]));
        let scalar = &classes.definitions[classes.ids[&id(2)]];
        assert!(
            scalar
                .methods
                .iter()
                .any(|method| method.name.as_str() == "add")
        );
    }

    #[test]
    fn async_iterator_keeps_sibling_diagnostics_in_source() {
        let iterator = "    async def __aiter__(self): yield 1\n";
        let invalid = "    def __unknown__(self): pass\n";
        for (methods, position) in [([iterator, invalid], 1), ([invalid, iterator], 0)] {
            let source = format!("class Stream:\n{}", methods.concat());
            let graph = SourceGraph::single(&source).unwrap();
            let module = &graph.modules[graph.entry];
            let class = module.parsed.suite()[0].as_class_def_stmt().unwrap();
            let expected = module.diagnostic(
                &class.body[position],
                "unsupported special method on a fixed-layout class",
            );
            assert_eq!(Classes::collect(&graph).err(), Some(expected));
        }
    }
}
