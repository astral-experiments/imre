//! A literal parser declaration determines a closed, read-only namespace layout.

use ruff_python_ast::visitor::{Visitor, walk_expr};
use ruff_text_size::TextSize;
use serde_json::{Value, json};

use super::*;
use crate::classes::{Class, DataclassOptions, Field};
use crate::modules::SourceModule;

#[derive(Default)]
pub(super) struct Parsers {
    declarations: HashSet<(ModuleId, TextSize)>,
    calls: HashMap<(ModuleId, TextSize), Parser>,
    namespaces: HashSet<usize>,
}

struct Parser {
    class: usize,
    schema: String,
    description: Option<ast::Expr>,
}

struct OptionField {
    name: String,
    annotation: ast::Expr,
    ty: Type,
    option: Value,
}

fn literal<'a>(expr: &'a ast::Expr, module: &SourceModule) -> Result<&'a str, String> {
    let ast::Expr::StringLiteral(value) = expr else {
        return Err(module.error(expr, "argparse declarations require literal strings"));
    };
    check_string(value, &module.source)?;
    Ok(value.value.to_str())
}

fn method<'a>(expr: &'a ast::Expr, receiver: &str, name: &str) -> Option<&'a ast::ExprCall> {
    let ast::Expr::Call(call) = expr else {
        return None;
    };
    let ast::Expr::Attribute(attribute) = &*call.func else {
        return None;
    };
    (attribute.attr.as_str() == name
        && matches!(&*attribute.value, ast::Expr::Name(name) if name.id == receiver))
    .then_some(call)
}

fn names(expr: &ast::Expr, name: &str) -> bool {
    matches!(expr, ast::Expr::Name(expr) if expr.id == name)
}

/// Validate all substitutions before emitting a native parser descriptor.
fn template(text: &str, fields: &[&str]) -> bool {
    let mut remaining = text;
    while let Some((_, rest)) = remaining.split_once('%') {
        if let Some(after) = rest.strip_prefix('%') {
            remaining = after;
        } else if let Some((name, after)) = rest.strip_prefix('(').and_then(|s| s.split_once(")s"))
            && fields.contains(&name)
        {
            remaining = after;
        } else {
            return false;
        }
    }
    true
}

fn keywords<'a>(
    call: &'a ast::ExprCall,
    allowed: &[&str],
    module: &SourceModule,
) -> Result<HashMap<&'a str, &'a ast::Expr>, String> {
    let mut values = HashMap::new();
    for keyword in &call.arguments.keywords {
        let name = keyword.arg.as_ref().map(ast::Identifier::as_str);
        if let Some(name) = name
            && allowed.contains(&name)
            && values.insert(name, &keyword.value).is_none()
        {
            continue;
        }
        return Err(module.error(keyword, "unsupported or repeated argparse keyword"));
    }
    Ok(values)
}

fn option(
    call: &ast::ExprCall,
    graph: &SourceGraph,
    id: ModuleId,
    types: &Types,
) -> Result<OptionField, String> {
    let module = &graph.modules[id];
    let [flag] = call.arguments.args.as_ref() else {
        return Err(module.error(call, "add_argument requires one literal long option"));
    };
    let flag_name = literal(flag, module)?;
    let Some(dest) = flag_name.strip_prefix("--") else {
        return Err(module.error(flag, "argparse requires a long option starting with '--'"));
    };
    let dest = dest.replace('-', "_");
    if !dest.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        || !dest.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
        || flag_name == "--help"
    {
        return Err(module.error(
            flag,
            "argparse option must specify an unambiguous ASCII field name",
        ));
    }
    let kw = keywords(
        call,
        &[
            "type", "nargs", "help", "action", "default", "required", "choices",
        ],
        module,
    )?;
    let mut element = Type::Str;
    if let Some(&ty) = kw.get("type") {
        if graph.resolve(id, ty) == Some(Binding::Intrinsic(Intrinsic::Path)) {
            element = Type::Path;
        } else if !names(ty, "str") {
            return Err(module.error(ty, "argparse type must be str or pathlib.Path"));
        }
    }
    let mut required = false;
    if let Some(&expr) = kw.get("required") {
        let ast::Expr::BooleanLiteral(value) = expr else {
            return Err(module.error(expr, "argparse required must be a literal bool"));
        };
        required = value.value;
    }
    let many = if let Some(&value) = kw.get("nargs") {
        if literal(value, module)? != "+" {
            return Err(module.error(value, "argparse supports nargs='+'"));
        }
        true
    } else {
        false
    };
    if let Some(&action) = kw.get("action") {
        if literal(action, module)? != "store_true"
            || kw.contains_key("type")
            || many
            || kw.contains_key("choices")
        {
            return Err(module.error(
                action,
                "argparse store_true takes no type, choices, or nargs",
            ));
        }
        element = Type::Bool;
    }
    let choices = if let Some(&choices) = kw.get("choices") {
        let items = match choices {
            ast::Expr::List(list) => list.elts.as_slice(),
            ast::Expr::Tuple(tuple) => tuple.elts.as_slice(),
            _ => {
                return Err(module.error(
                    choices,
                    "argparse choices require a literal list or tuple of strings",
                ));
            }
        };
        if element != Type::Str || items.is_empty() {
            return Err(module.error(
                choices,
                "argparse choices require nonempty string choices and type str",
            ));
        }
        items
            .iter()
            .map(|item| literal(item, module))
            .collect::<Result<Vec<_>, _>>()?
    } else {
        Vec::new()
    };
    let default = match kw.get("default").copied() {
        None if element == Type::Bool => Some("False"),
        None | Some(ast::Expr::NoneLiteral(_)) => None,
        Some(expr @ ast::Expr::StringLiteral(_)) if !many && element != Type::Bool => {
            Some(literal(expr, module)?)
        }
        Some(ast::Expr::BooleanLiteral(value)) if element == Type::Bool => {
            Some(if value.value { "True" } else { "False" })
        }
        Some(ast::Expr::Call(path))
            if !many
                && element == Type::Path
                && graph.resolve(id, &path.func) == Some(Binding::Intrinsic(Intrinsic::Path))
                && path.arguments.keywords.is_empty()
                && let [value] = path.arguments.args.as_ref() =>
        {
            Some(literal(value, module)?)
        }
        Some(value) => {
            return Err(module.error(
                value,
                "argparse default must be None or a literal of the declared option type",
            ));
        }
    };
    let path_default = matches!(kw.get("default"), Some(ast::Expr::Call(_)));
    let help = kw
        .get("help")
        .map(|&value| literal(value, module))
        .transpose()?
        .unwrap_or("");
    if !template(
        help,
        &[
            "prog", "dest", "type", "default", "choices", "required", "nargs",
        ],
    ) {
        return Err(module.error(call, "unsupported argparse help substitution"));
    }
    let converter = kw
        .get("type")
        .map(|_| if element == Type::Path { "Path" } else { "str" });
    let kind = match element {
        Type::Str => "str",
        Type::Path => "path",
        Type::Bool => "bool",
        _ => unreachable!(),
    };
    let optional = !required && default.is_none();
    let ty = if many {
        types.list(element)
    } else if !choices.is_empty() {
        let values = choices
            .iter()
            .map(|s| s.to_string())
            .chain(default.map(str::to_owned))
            .collect();
        types.string_literal(values)
    } else {
        element
    };
    Ok(OptionField {
        name: dest,
        annotation: flag.clone(),
        ty: if optional { types.optional(ty) } else { ty },
        option: json!({ "flag": flag_name, "kind": kind, "many": many, "required": required,
            "choices": choices, "converter": converter, "default": default, "path_default": path_default, "help": help, "optional": optional }),
    })
}

struct ContainsName<'a>(&'a str, bool);

impl<'ast> Visitor<'ast> for ContainsName<'_> {
    fn visit_expr(&mut self, node: &'ast ast::Expr) {
        self.1 |= names(node, self.0);
        if !self.1 {
            walk_expr(self, node)
        }
    }
}

impl Parsers {
    pub(super) fn namespace(&self, class: usize) -> bool {
        self.namespaces.contains(&class)
    }

    pub(super) fn declaration(&self, module: ModuleId, statement: &ast::Stmt) -> bool {
        self.declarations.contains(&(module, statement.start()))
    }

    pub(super) fn collect(
        graph: &SourceGraph,
        types: &Types,
        classes: &mut Classes,
        layouts: &mut Vec<Vec<Type>>,
    ) -> Result<Self, String> {
        let mut parsers = Self::default();
        for (module_id, module) in graph.modules.iter().enumerate() {
            for (statement, node) in module.parsed.syntax().body.iter().enumerate() {
                let ast::Stmt::FunctionDef(function) = node else {
                    continue;
                };
                for (start, stmt) in function.body.iter().enumerate() {
                    let ast::Stmt::Assign(assign) = stmt else {
                        continue;
                    };
                    let [ast::Expr::Name(name)] = assign.targets.as_slice() else {
                        continue;
                    };
                    let ast::Expr::Call(ctor) = &*assign.value else {
                        continue;
                    };
                    if graph.resolve(module_id, &ctor.func)
                        != Some(Binding::Intrinsic(Intrinsic::ArgumentParser))
                    {
                        continue;
                    }
                    if !ctor.arguments.args.is_empty() {
                        return Err(module
                            .error(ctor, "ArgumentParser requires named literal configuration"));
                    }
                    let kw = keywords(ctor, &["description", "formatter_class", "prog"], module)?;
                    let raw = if let Some(&value) = kw.get("formatter_class") {
                        if graph.resolve(module_id, value)
                            != Some(Binding::Intrinsic(Intrinsic::RawDescriptionHelpFormatter))
                        {
                            return Err(module
                                .error(value, "argparse supports RawDescriptionHelpFormatter"));
                        }
                        true
                    } else {
                        false
                    };
                    let prog = if let Some(&value) = kw.get("prog") {
                        literal(value, module)?
                    } else {
                        graph.modules[graph.entry]
                            .path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap()
                    };
                    let description = kw.get("description").copied();
                    if let Some(expr) = description
                        && !matches!(
                            expr,
                            ast::Expr::StringLiteral(_) | ast::Expr::NoneLiteral(_)
                        )
                        && !names(expr, "__doc__")
                    {
                        return Err(module
                            .error(expr, "argparse description requires a literal or __doc__"));
                    }
                    let builder = name.id.as_str();
                    if reserved(builder)
                        || module.bindings.contains_key(builder)
                        || function
                            .parameters
                            .iter_non_variadic_params()
                            .any(|param| param.name() == builder)
                    {
                        return Err(
                            module.error(name, "argparse builder requires a fresh local name")
                        );
                    }
                    let mut before = ContainsName(builder, false);
                    before.visit_body(&function.body[..start]);
                    let mut options = Vec::new();
                    let mut names = HashSet::new();
                    let mut index = start + 1;
                    while let Some(ast::Stmt::Expr(stmt)) = function.body.get(index)
                        && let Some(add) = method(&stmt.value, builder, "add_argument")
                    {
                        let field = option(add, graph, module_id, types)?;
                        if !names.insert(field.name.clone()) {
                            return Err(module.error(
                                add,
                                "argparse arguments require unique destination fields",
                            ));
                        }
                        options.push(field);
                        index += 1;
                    }
                    let parse = match function.body.get(index) {
                        Some(ast::Stmt::Assign(stmt)) if stmt.targets.len() == 1 && stmt.targets[0].as_name_expr().is_some() => Some(&*stmt.value),
                        Some(ast::Stmt::Return(stmt)) => stmt.value.as_deref(),
                        _ => None,
                    }.and_then(|expr| method(expr, builder, "parse_args"))
                        .ok_or_else(|| module.error(ctor, "literal add_argument declarations must be followed by an assignment or return of parse_args()"))?;
                    if !parse.arguments.args.is_empty() || !parse.arguments.keywords.is_empty() {
                        return Err(
                            module.error(parse, "argparse parse_args() reads process arguments")
                        );
                    }
                    before.visit_body(&function.body[index + 1..]);
                    if before.1 {
                        return Err(module.error(
                            name,
                            "argparse builder cannot escape its declaration or be rebound",
                        ));
                    }
                    let class = classes.definitions.len();
                    let fields = options
                        .iter()
                        .map(|field| Field {
                            name: field.name.clone(),
                            annotation: field.annotation.clone(),
                            default: None,
                            kw_only: false,
                            repr: true,
                            compare: true,
                        })
                        .collect();
                    layouts.push(options.iter().map(|field| field.ty).collect());
                    let range = ctor.range();
                    classes.definitions.push(Class {
                        name: "Namespace".into(),
                        definition: DefinitionId {
                            module: module_id,
                            statement,
                        },
                        node: ast::StmtClassDef {
                            node_index: ast::AtomicNodeIndex::NONE,
                            range,
                            decorator_list: Default::default(),
                            name: ast::Identifier::new("Namespace", range),
                            type_params: None,
                            arguments: None,
                            body: Default::default(),
                        },
                        fields,
                        methods: Vec::new(),
                        method_kinds: Vec::new(),
                        dataclass: Some(DataclassOptions {
                            init: false,
                            eq: true,
                            repr: true,
                            frozen: true,
                            kw_only: false,
                        }),
                        enum_members: None,
                        initializer: None,
                        generated_initializer: false,
                    });
                    parsers.namespaces.insert(class);
                    for stmt in &function.body[start..index] {
                        parsers.declarations.insert((module_id, stmt.start()));
                    }
                    let options = options
                        .into_iter()
                        .map(|field| field.option)
                        .collect::<Vec<_>>();
                    let schema = json!({"prog": prog, "raw": raw, "options": options}).to_string();
                    parsers.calls.insert(
                        (module_id, parse.start()),
                        Parser {
                            class,
                            schema,
                            description: description.cloned(),
                        },
                    );
                }
            }
        }
        Ok(parsers)
    }
}

impl Checker<'_> {
    pub(super) fn parse_args(&mut self, call: &ast::ExprCall) -> Result<Option<Expr>, String> {
        let Some(parser) = self.classes.parsers.calls.get(&(self.module, call.start())) else {
            return Ok(None);
        };
        let description = match &parser.description {
            Some(expr) if !matches!(expr, ast::Expr::NoneLiteral(_)) => {
                let value = self.expr(expr)?;
                if let ExprKind::Str(text) = &value.kind
                    && text.contains("%(prog)")
                    && !template(text, &["prog"])
                {
                    return Err(error(
                        self.source,
                        expr,
                        "unsupported argparse description substitution",
                    ));
                }
                if value.ty == Type::Str {
                    value
                } else if value.ty == Type::None {
                    Expr {
                        ty: Type::Str,
                        kind: ExprKind::Str(String::new()),
                    }
                } else {
                    return Err(error(
                        self.source,
                        expr,
                        "argparse description requires str or None",
                    ));
                }
            }
            _ => Expr {
                ty: Type::Str,
                kind: ExprKind::Str(String::new()),
            },
        };
        Ok(Some(Expr {
            ty: Type::Class(parser.class),
            kind: ExprKind::Builtin(
                Builtin::ParseArgs,
                vec![
                    Expr {
                        ty: Type::Str,
                        kind: ExprKind::Str(parser.schema.clone()),
                    },
                    description,
                    Expr {
                        ty: self.types.list(Type::Str),
                        kind: ExprKind::Builtin(Builtin::SysArgv, Vec::new()),
                    },
                ],
            ),
        }))
    }

    /// An immutable optional field stays present in this arm while its local
    /// receiver keeps the same binding, even when another alias survives calls.
    pub(super) fn namespace_guard(
        &self,
        test: &ast::Expr,
        body: &[ast::Stmt],
    ) -> Option<(usize, usize)> {
        let ast::Expr::Attribute(attr) = test else {
            return None;
        };
        let name = attr.value.as_name_expr()?.id.as_str();
        let slot = *self.names.get(name)?;
        let Type::Class(class) = self.locals[slot] else {
            return None;
        };
        if !self.classes.parsers.namespace(class)
            || narrowing::assigned_names(body)
                .iter()
                .any(|written| written == name)
        {
            return None;
        }
        let field = self.classes.declarations.definitions[class]
            .fields
            .iter()
            .position(|field| field.name == attr.attr.as_str())?;
        self.types.payload(self.classes.fields[class][field])?;
        Some((slot, field))
    }
}
