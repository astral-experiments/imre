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
    /// Child layouts share the complete parent prefix and add their own options.
    commands: HashMap<usize, (String, HashMap<String, usize>)>,
}

struct Parser {
    class: usize,
    schema: String,
    children: Vec<usize>,
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

fn method_receiver<'a>(expr: &'a ast::Expr, name: &str) -> Option<(&'a str, &'a ast::ExprCall)> {
    let ast::Expr::Call(call) = expr else {
        return None;
    };
    let ast::Expr::Attribute(attribute) = &*call.func else {
        return None;
    };
    let receiver = attribute.value.as_name_expr()?;
    (attribute.attr.as_str() == name).then_some((receiver.id.as_str(), call))
}

fn method<'a>(expr: &'a ast::Expr, receiver: &str, name: &str) -> Option<&'a ast::ExprCall> {
    let (owner, call) = method_receiver(expr, name)?;
    (owner == receiver).then_some(call)
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

/// Resolve a sealed enum independently of its spelling or the importing module.
fn enum_class(
    classes: &Classes,
    graph: &SourceGraph,
    module: ModuleId,
    expr: &ast::Expr,
) -> Option<usize> {
    let Binding::Definition(definition) = graph.resolve(module, expr)? else {
        return None;
    };
    let class = classes.ids.get(&definition).copied()?;
    classes.definitions[class]
        .enum_members
        .as_ref()
        .map(|_| class)
}

fn option(
    call: &ast::ExprCall,
    graph: &SourceGraph,
    id: ModuleId,
    types: &Types,
    classes: &Classes,
) -> Result<OptionField, String> {
    let module = &graph.modules[id];
    let Some((flag, rest)) = call.arguments.args.split_first() else {
        return Err(module.error(
            call,
            "add_argument requires a literal option or positional name",
        ));
    };
    let flag_name = literal(flag, module)?;
    let positional = !flag_name.starts_with('-');
    let aliases = rest
        .iter()
        .map(|expr| literal(expr, module))
        .collect::<Result<Vec<_>, _>>()?;
    if positional && (!aliases.is_empty() || flag_name.contains('-')) {
        return Err(module.error(flag, "argparse positional arguments have one name"));
    }
    let spellings = std::iter::once(flag_name).chain(aliases.iter().copied());
    let dest = if positional {
        flag_name
    } else {
        spellings
            .clone()
            .find_map(|s| s.strip_prefix("--"))
            .unwrap_or(flag_name.trim_start_matches('-'))
    }
    .replace('-', "_");
    let mut seen = HashSet::new();
    if !dest.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        || !dest.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
        || spellings.into_iter().any(|s| {
            !seen.insert(s)
                || !positional
                    && (matches!(s, "-h" | "--help")
                        || !(s.strip_prefix("--").is_some_and(|n| {
                            !n.is_empty()
                                && n.bytes()
                                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
                        }) || s.len() == 2
                            && s.starts_with('-')
                            && s.as_bytes()[1].is_ascii_alphabetic()))
        })
    {
        return Err(module.error(
            flag,
            "argparse arguments require distinct ASCII field names and flags",
        ));
    }
    let kw = keywords(
        call,
        &[
            "type", "nargs", "help", "action", "default", "const", "required", "choices",
        ],
        module,
    )?;
    if positional && kw.contains_key("required") {
        return Err(module.error(call, "argparse positionals do not accept required"));
    }
    let mut element = Type::Str;
    if let Some(&ty) = kw.get("type") {
        if graph.resolve(id, ty) == Some(Binding::Intrinsic(Intrinsic::Path)) {
            element = Type::Path;
        } else if !names(ty, "str") {
            return Err(module.error(ty, "argparse type must be str or pathlib.Path"));
        }
    }
    let nargs = match kw.get("nargs").copied() {
        None | Some(ast::Expr::NoneLiteral(_)) => json!("one"),
        Some(ast::Expr::StringLiteral(s)) => match s.value.to_str() {
            "?" => json!("optional"),
            "*" => json!("zero_or_more"),
            "+" => json!("one_or_more"),
            _ => return Err(module.error(call, "unsupported argparse nargs")),
        },
        Some(ast::Expr::NumberLiteral(n))
            if let ast::Number::Int(value) = &n.value
                && let Some(count) = value.as_i64().and_then(|n| usize::try_from(n).ok())
                && count > 0 =>
        {
            json!({"fixed": count})
        }
        Some(value) => {
            return Err(module.error(
                value,
                "argparse nargs requires a positive count, '?', '*' or '+'",
            ));
        }
    };
    let many = nargs != "one" && nargs != "optional";
    let mut required = positional && nargs != "optional" && nargs != "zero_or_more";
    if let Some(&expr) = kw.get("required") {
        let ast::Expr::BooleanLiteral(value) = expr else {
            return Err(module.error(expr, "argparse required must be a literal bool"));
        };
        required = value.value;
    }
    if let Some(&action) = kw.get("action") {
        match literal(action, module)? {
            "store" => {}
            "store_true"
                if !positional
                    && !kw.contains_key("type")
                    && !kw.contains_key("nargs")
                    && !kw.contains_key("const")
                    && !kw.contains_key("choices") =>
            {
                element = Type::Bool
            }
            _ => return Err(module.error(action, "unsupported argparse action or action keywords")),
        }
    }
    let constant = match kw.get("const").copied() {
        None if element == Type::Bool => Some("True"),
        None | Some(ast::Expr::NoneLiteral(_)) => None,
        Some(value) if nargs == "optional" => Some(literal(value, module)?),
        Some(value) => return Err(module.error(value, "argparse const requires nargs='?'")),
    };
    let choices = if let Some(&choices) = kw.get("choices") {
        let choices = match choices {
            ast::Expr::ListComp(comp)
                if let [clause] = comp.generators.as_slice()
                    && !clause.is_async
                    && clause.ifs.is_empty()
                    && let ast::Expr::Name(target) = &clause.target
                    && let ast::Expr::Attribute(attr) = &*comp.elt
                    && attr.attr.as_str() == "value"
                    && names(&attr.value, target.id.as_str())
                    && let Some(class) = enum_class(classes, graph, id, &clause.iter) =>
            {
                classes.definitions[class]
                    .enum_members
                    .as_ref()
                    .unwrap()
                    .values
                    .iter()
                    .map(|(_, value)| value.as_str())
                    .collect()
            }
            items => {
                let items = match items {
                    ast::Expr::List(list) => list.elts.as_slice(),
                    ast::Expr::Tuple(tuple) => tuple.elts.as_slice(),
                    _ => return Err(module.error(
                        choices,
                        "argparse choices require literal strings or the values of a sealed enum",
                    )),
                };
                items
                    .iter()
                    .map(|item| literal(item, module))
                    .collect::<Result<Vec<_>, _>>()?
            }
        };
        if element != Type::Str || choices.is_empty() {
            return Err(module.error(
                call,
                "argparse choices require nonempty string choices and type str",
            ));
        }
        choices
    } else {
        Vec::new()
    };
    let mut enum_default = None;
    let enum_default_text;
    let default = match kw.get("default").copied() {
        None if element == Type::Bool => Some("False"),
        None | Some(ast::Expr::NoneLiteral(_)) => None,
        Some(expr @ ast::Expr::StringLiteral(_))
            if element == Type::Str || !many && element == Type::Path =>
        {
            Some(literal(expr, module)?)
        }
        Some(ast::Expr::BooleanLiteral(value)) if element == Type::Bool => {
            Some(if value.value { "True" } else { "False" })
        }
        Some(ast::Expr::Attribute(attr))
            if !many
                && element == Type::Str
                && let Some(class) = enum_class(classes, graph, id, &attr.value)
                && let Some(members) = &classes.definitions[class].enum_members
                && members.plain
                && let Some(&member) = members.names.get(attr.attr.as_str()) =>
        {
            // The parser stores a tagged enum only for an omitted nominal default.
            // Explicit arguments remain strings, even if they name this same member.
            let first = classes.definitions[..class]
                .iter()
                .filter_map(|c| c.enum_members.as_ref())
                .map(|members| members.values.len())
                .sum::<usize>()
                + 1;
            enum_default = Some((class, first + member));
            enum_default_text = format!(
                "{}.{}",
                classes.definitions[class].name, members.values[member].0
            );
            Some(enum_default_text.as_str())
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
        Some(value) => return Err(module.error(
            value,
            "argparse default must be None, a fixed enum member, or a literal of the option type",
        )),
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
            "prog", "dest", "type", "default", "const", "choices", "required", "nargs",
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
    let optional = (!required && default.is_none() && !(positional && nargs == "zero_or_more"))
        || (!positional && nargs == "optional" && constant.is_none());
    let list_default = many && !required && default.is_some();
    let ty = if many {
        let list = types.list(element);
        if list_default {
            types.union(&[Type::Str, list]).ok_or_else(|| {
                module.error(call, "argparse list and default need a representable union")
            })?
        } else {
            list
        }
    } else if let Some((class, _)) = enum_default {
        types
            .union(&[Type::Str, Type::Enum(class)])
            .ok_or_else(|| module.error(call, "argparse requires a sealed string/enum field"))?
    } else if !choices.is_empty() {
        let values = choices
            .iter()
            .map(|s| s.to_string())
            .chain(default.map(str::to_owned))
            .chain(constant.map(str::to_owned))
            .collect();
        types.string_literal(values)
    } else {
        element
    };
    Ok(OptionField {
        name: dest,
        annotation: flag.clone(),
        ty: if optional { types.optional(ty) } else { ty },
        option: json!({ "flag": flag_name, "aliases": aliases, "enum_default": enum_default.map(|(_, tag)| tag), "kind": kind, "nargs": nargs, "constant": constant, "list_default":list_default, "required": required,
            "choices": choices, "converter": converter, "default": default, "path_default": path_default, "help": help, "optional": optional }),
    })
}

struct Subcommand {
    name: String,
    help: Option<String>,
    options: Vec<OptionField>,
    groups: Vec<Value>,
}

fn argument_group(call: &ast::ExprCall, module: &SourceModule) -> Result<Value, String> {
    let kw = keywords(call, &["title", "description"], module)?;
    if call.arguments.args.len() > 2 {
        return Err(module.error(call, "argparse groups accept a title and a description"));
    }
    let text = |index, name| match (call.arguments.args.get(index), kw.get(name).copied()) {
        (Some(_), Some(_)) => Err(module.error(call, "repeated argparse group argument")),
        (Some(value), None) | (None, Some(value))
            if !matches!(value, ast::Expr::NoneLiteral(_)) =>
        {
            literal(value, module).map(Some)
        }
        _ => Ok(None),
    };
    let title = text(0, "title")?;
    let description = text(1, "description")?;
    if description.is_some_and(|s| s.contains("%(prog)") && !template(s, &["prog"])) {
        return Err(module.error(call, "unsupported argparse group description substitution"));
    }
    Ok(json!({"title": title, "description": description}))
}

/// Each descriptor uses the same order for storage, parsing, and destruction.
fn namespace_class(
    classes: &mut Classes,
    layouts: &mut Vec<Vec<Type>>,
    definition: DefinitionId,
    range: TextRange,
    fields: &[&OptionField],
) -> usize {
    let class = classes.definitions.len();
    layouts.push(fields.iter().map(|field| field.ty).collect());
    classes.definitions.push(Class {
        exception_aliases: Default::default(),
        name: "Namespace".into(),
        definition,
        node: ast::StmtClassDef {
            node_index: ast::AtomicNodeIndex::NONE,
            range,
            decorator_list: Default::default(),
            name: ast::Identifier::new("Namespace", range),
            type_params: None,
            arguments: None,
            body: Default::default(),
        },
        fields: fields
            .iter()
            .map(|field| Field {
                module: definition.module,
                name: field.name.clone(),
                annotation: field.annotation.clone(),
                default: None,
                kw_only: false,
                repr: true,
                compare: true,
            })
            .collect(),
        methods: Vec::new(),
        method_kinds: Vec::new(),
        method_origins: Vec::new(),
        bases: Vec::new(),
        abstract_methods: Default::default(),
        toml_hash: false,
        abc: false,
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
    class
}

fn assigned_call<'a>(
    statement: Option<&'a ast::Stmt>,
    receiver: &str,
    operation: &str,
) -> Option<(&'a ast::ExprName, &'a ast::ExprCall)> {
    let ast::Stmt::Assign(assign) = statement? else {
        return None;
    };
    let [ast::Expr::Name(name)] = assign.targets.as_slice() else {
        return None;
    };
    Some((name, method(&assign.value, receiver, operation)?))
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

    pub(super) fn child_of(&self, child: usize, parent: usize) -> bool {
        self.commands
            .get(&parent)
            .is_some_and(|(_, children)| children.values().any(|&value| value == child))
    }

    /// Guards may specialize a value for one command. A shared handler or
    /// assignment can widen siblings to exactly their common record prefix.
    pub(super) fn common_namespace(&self, left: Type, right: Type) -> Option<Type> {
        let (Type::Class(left), Type::Class(right)) = (left, right) else {
            return None;
        };
        self.commands.iter().find_map(|(&parent, (_, children))| {
            let belongs = |class| class == parent || children.values().any(|&child| child == class);
            (belongs(left) && belongs(right)).then_some(Type::Class(parent))
        })
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
            let functions =
                module
                    .parsed
                    .syntax()
                    .body
                    .iter()
                    .enumerate()
                    .flat_map(|(statement, node)| {
                        let declarations = match node {
                            ast::Stmt::ClassDef(class) => class.body.as_ref(),
                            _ => std::slice::from_ref(node),
                        };
                        declarations.iter().filter_map(move |node| match node {
                            ast::Stmt::FunctionDef(function) => Some((statement, function)),
                            _ => None,
                        })
                    });
            for (statement, function) in functions {
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
                    let mut builders = Vec::<String>::new();
                    let register =
                        |name: &ast::ExprName, builders: &[String]| -> Result<(), String> {
                            let name = name.id.as_str();
                            if reserved(name)
                                || module.bindings.contains_key(name)
                                || function
                                    .parameters
                                    .iter_non_variadic_params()
                                    .any(|param| param.name() == name)
                                || builders.iter().any(|builder| builder == name)
                            {
                                return Err(module
                                    .error(ctor, "argparse builder requires a fresh local name"));
                            }
                            Ok(())
                        };
                    register(name, &builders)?;
                    builders.push(builder.to_owned());
                    let read_options =
                        |builder: &str,
                         index: &mut usize,
                         mut names: HashSet<String>,
                         builders: &mut Vec<String>|
                         -> Result<(Vec<OptionField>, Vec<Value>), String> {
                            let mut options: Vec<OptionField> = Vec::new();
                            let mut spellings = HashSet::new();
                            let mut groups = Vec::new();
                            let mut group_names = HashMap::new();
                            while let Some(stmt) = function.body.get(*index) {
                                if let Some((group, call)) =
                                    assigned_call(Some(stmt), builder, "add_argument_group")
                                {
                                    register(group, builders)?;
                                    builders.push(group.id.to_string());
                                    group_names.insert(group.id.as_str(), groups.len());
                                    groups.push(argument_group(call, module)?);
                                    *index += 1;
                                    continue;
                                }
                                let ast::Stmt::Expr(stmt) = stmt else { break };
                                let Some((owner, add)) =
                                    method_receiver(&stmt.value, "add_argument")
                                else {
                                    break;
                                };
                                let group = group_names.get(owner);
                                if owner != builder && group.is_none() {
                                    break;
                                }
                                let mut field = option(add, graph, module_id, types, classes)?;
                                if let Some(group) = group {
                                    field.option["group"] = json!(group);
                                }
                                for value in &add.arguments.args {
                                    let name = literal(value, module)?;
                                    if !spellings.insert(name) {
                                        return Err(module.error(
                                            value,
                                            "argparse option spellings must be unique",
                                        ));
                                    }
                                }
                                if !names.insert(field.name.clone()) {
                                    return Err(module.error(
                                        add,
                                        "argparse arguments require unique destination fields",
                                    ));
                                }
                                options.push(field);
                                *index += 1;
                            }
                            Ok((options, groups))
                        };
                    let mut index = start + 1;
                    let (options, groups) =
                        read_options(builder, &mut index, HashSet::new(), &mut builders)?;
                    let mut root_names = options
                        .iter()
                        .map(|field| field.name.clone())
                        .collect::<HashSet<_>>();
                    let mut children = Vec::new();
                    let mut command = None;
                    if let Some((group, add)) =
                        assigned_call(function.body.get(index), builder, "add_subparsers")
                    {
                        register(group, &builders)?;
                        builders.push(group.id.to_string());
                        let kw = keywords(add, &["dest", "required"], module)?;
                        let Some(dest) = kw.get("dest") else {
                            return Err(
                                module.error(add, "subparsers require a literal destination")
                            );
                        };
                        let dest = literal(dest, module)?;
                        if !add.arguments.args.is_empty()
                            || !matches!(kw.get("required"), Some(ast::Expr::BooleanLiteral(value)) if value.value)
                            || !dest.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
                            || !dest
                                .bytes()
                                .all(|ch| ch.is_ascii_alphanumeric() || ch == b'_')
                            || !root_names.insert(dest.to_owned())
                        {
                            return Err(module.error(add, "argparse supports one required subcommand with a distinct destination"));
                        }
                        index += 1;
                        while let Some((child, call)) =
                            assigned_call(function.body.get(index), group.id.as_str(), "add_parser")
                        {
                            register(child, &builders)?;
                            builders.push(child.id.to_string());
                            let [name] = call.arguments.args.as_ref() else {
                                return Err(module.error(
                                    call,
                                    "add_parser requires one literal subcommand name",
                                ));
                            };
                            let name = literal(name, module)?;
                            if name.is_empty()
                                || name.starts_with('-')
                                || name.chars().any(char::is_whitespace)
                                || children.iter().any(|child: &Subcommand| child.name == name)
                            {
                                return Err(module.error(
                                    call,
                                    "argparse subcommand names must be unique literal words",
                                ));
                            }
                            let kw = keywords(call, &["help"], module)?;
                            let help = kw
                                .get("help")
                                .map(|value| literal(value, module))
                                .transpose()?;
                            if help.is_some_and(|help| !template(help, &["prog"])) {
                                return Err(module.error(
                                    call,
                                    "unsupported argparse subcommand help substitution",
                                ));
                            }
                            index += 1;
                            let (options, groups) = read_options(
                                child.id.as_str(),
                                &mut index,
                                root_names.clone(),
                                &mut builders,
                            )?;
                            children.push(Subcommand {
                                name: name.into(),
                                help: help.map(str::to_owned),
                                options,
                                groups,
                            });
                        }
                        if children.is_empty() {
                            return Err(module.error(add, "argparse subparsers require at least one literal add_parser declaration"));
                        }
                        command = Some(OptionField {
                            name: dest.to_owned(),
                            annotation: ast::Expr::from(group.clone()),
                            ty: types
                                .string_literal(children.iter().map(|c| c.name.clone()).collect()),
                            option: Value::Null,
                        });
                    }
                    let parse = match function.body.get(index) {
                        Some(ast::Stmt::Expr(stmt)) => Some(&*stmt.value),
                        Some(ast::Stmt::Assign(stmt)) if stmt.targets.len() == 1 && stmt.targets[0].as_name_expr().is_some() => Some(&*stmt.value),
                        Some(ast::Stmt::Return(stmt)) => stmt.value.as_deref(),
                        _ => None,
                    }.and_then(|expr| method(expr, builder, "parse_args"))
                        .ok_or_else(|| module.error(ctor, "literal add_argument declarations must be followed by an assignment or return of parse_args()"))?;
                    for builder in builders {
                        let mut outside = ContainsName(&builder, false);
                        outside.visit_body(&function.body[..start]);
                        outside.visit_body(&function.body[index + 1..]);
                        if outside.1 {
                            return Err(module.error(
                                name,
                                "argparse builder cannot escape its declaration or be rebound",
                            ));
                        }
                    }
                    let definition = DefinitionId {
                        module: module_id,
                        statement,
                    };
                    let root = options.iter().chain(command.iter()).collect::<Vec<_>>();
                    let class = namespace_class(classes, layouts, definition, ctor.range(), &root);
                    parsers.namespaces.insert(class);
                    let mut child_classes = Vec::new();
                    let mut child_schemas = Vec::new();
                    let mut tags = HashMap::new();
                    for child in &children {
                        let fields = root
                            .iter()
                            .copied()
                            .chain(&child.options)
                            .collect::<Vec<_>>();
                        let variant =
                            namespace_class(classes, layouts, definition, ctor.range(), &fields);
                        parsers.namespaces.insert(variant);
                        child_classes.push(variant);
                        tags.insert(child.name.clone(), variant);
                        child_schemas.push(json!({
                            "name": child.name, "help": child.help,
                            "raw": false,
                            "options": child.options.iter().map(|f| &f.option).collect::<Vec<_>>(),
                            "groups": child.groups,
                        }));
                    }
                    let subcommands = command.as_ref().map(|field| {
                        parsers.commands.insert(class, (field.name.clone(), tags));
                        json!({"dest": field.name, "choices": child_schemas})
                    });
                    for stmt in &function.body[start..index] {
                        parsers.declarations.insert((module_id, stmt.start()));
                    }
                    let options = options
                        .into_iter()
                        .map(|field| field.option)
                        .collect::<Vec<_>>();
                    let schema = json!({"prog": prog, "raw": raw, "options": options, "groups": groups, "subcommands": subcommands}).to_string();
                    parsers.calls.insert(
                        (module_id, parse.start()),
                        Parser {
                            class,
                            children: child_classes,
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
        let explicit = match (
            call.arguments.args.as_ref(),
            call.arguments.keywords.as_ref(),
        ) {
            ([], []) => None,
            ([value], []) => Some(value),
            ([], [keyword]) if keyword.arg.as_ref().is_some_and(|name| name == "args") => {
                Some(&keyword.value)
            }
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "argparse parse_args accepts one argument list",
                ));
            }
        };
        let explicit = explicit.filter(|value| !matches!(value, ast::Expr::NoneLiteral(_)));
        let argv_type = self.types.list(Type::Str);
        let argv = match explicit {
            Some(ast::Expr::Tuple(tuple)) => Expr {
                ty: argv_type,
                kind: ExprKind::List(
                    tuple
                        .elts
                        .iter()
                        .map(|value| self.expr_expected(value, Type::Str))
                        .collect::<Result<_, _>>()?,
                ),
            },
            Some(value) => self.expr_expected(value, argv_type)?,
            None => Expr {
                ty: argv_type,
                kind: ExprKind::Builtin(Builtin::SysArgv, Vec::new()),
            },
        };
        Ok(Some(Expr {
            ty: Type::Class(parser.class),
            kind: ExprKind::Builtin(
                Builtin::ParseArgs {
                    children: parser.children.clone(),
                    process_argv: explicit.is_none(),
                },
                vec![
                    Expr {
                        ty: Type::Str,
                        kind: ExprKind::Str(parser.schema.clone()),
                    },
                    description,
                    argv,
                ],
            ),
        }))
    }

    pub(super) fn narrow_command(&mut self, attribute: &ast::ExprAttribute, value: &str) {
        let Some(name) = attribute.value.as_name_expr() else {
            return;
        };
        let Some(&slot) = self.names.get(name.id.as_str()) else {
            return;
        };
        let Type::Class(class) = self.locals[slot] else {
            return;
        };
        if let Some((field, children)) = self.classes.parsers.commands.get(&class)
            && field == attribute.attr.as_str()
            && let Some(&variant) = children.get(value)
        {
            self.narrowed.insert_mut(slot, Type::Class(variant));
        }
    }

    pub(super) fn enter_namespace_guards(&mut self, test: &ast::Expr, body: &[ast::Stmt]) {
        if let ast::Expr::BoolOp(value) = test
            && value.op == ast::BoolOp::And
        {
            // A later operand may rebind the receiver even if the body does not.
            let mut reassigned = false;
            for operand in value.values.iter().rev() {
                if !reassigned && let Some(guard) = self.namespace_guard(operand, body) {
                    self.namespace_guards.push(guard);
                }
                if !matches!(operand, ast::Expr::Attribute(attr) if attr.value.is_name_expr()) {
                    reassigned = true
                }
            }
        } else if let Some(guard) = self.namespace_guard(test, body) {
            self.namespace_guards.push(guard);
        }
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
        let Type::Class(class) = self
            .narrowed
            .get(&slot)
            .copied()
            .unwrap_or(self.locals[slot])
        else {
            return None;
        };
        let frozen = self.classes.declarations.definitions[class]
            .dataclass
            .is_some_and(|d| d.frozen);
        if !(self.classes.parsers.namespace(class) || frozen)
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
