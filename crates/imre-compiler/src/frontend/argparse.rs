//! A literal parser declaration determines a closed, read-only namespace layout.

use ruff_python_ast::visitor::{Visitor, walk_expr};
use ruff_text_size::TextSize;
use serde_json::{Value, json};

use super::*;
use crate::classes::{Class, DataclassOptions, Field};
use crate::modules::SourceModule;

mod registration;

#[derive(Default)]
pub(super) struct Parsers {
    declarations: HashSet<(ModuleId, TextSize)>,
    calls: HashMap<(ModuleId, TextSize), Parser>,
    registration_errors: HashMap<(ModuleId, TextSize), Expr>,
    namespaces: HashSet<usize>,
    /// Child layouts share the complete parent prefix and add their own options.
    commands: HashMap<usize, CommandVariants>,
}

struct CommandVariants {
    destination: Option<String>,
    by_name: HashMap<String, usize>,
}

#[derive(Clone)]
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

/// Encode signed numeric literals without evaluating declaration expressions.
fn numeric_literal(expr: &ast::Expr, ty: Type) -> Option<String> {
    let (expr, neg) = match expr {
        ast::Expr::UnaryOp(op) if matches!(op.op, ast::UnaryOp::UAdd | ast::UnaryOp::USub) => {
            (&*op.operand, op.op == ast::UnaryOp::USub)
        }
        expr => (expr, false),
    };
    let ast::Expr::NumberLiteral(number) = expr else {
        return None;
    };
    match (&number.value, ty) {
        (ast::Number::Int(value), Type::Int) => {
            let value = value.to_string();
            Some(if neg && value != "0" {
                format!("-{value}")
            } else {
                value
            })
        }
        (ast::Number::Float(value), Type::Float) => {
            Some(format!("{:?}", if neg { -*value } else { *value }))
        }
        _ => None,
    }
}

/// Keep literal integer ranges compact and preserve arbitrary-precision endpoints.
fn range_choices(call: &ast::ExprCall) -> Option<[String; 3]> {
    if !call.arguments.keywords.is_empty() {
        return None;
    }
    let args = call
        .arguments
        .args
        .iter()
        .map(|arg| numeric_literal(arg, Type::Int))
        .collect::<Option<Vec<_>>>()?;
    match args.as_slice() {
        [stop] => Some(["0".into(), stop.clone(), "1".into()]),
        [start, stop] => Some([start.clone(), stop.clone(), "1".into()]),
        [start, stop, step] if step != "0" => Some([start.clone(), stop.clone(), step.clone()]),
        _ => None,
    }
}

/// Prefixes affect both the option grammar and the implicit help action.
struct Flags<'a> {
    prefixes: &'a str,
    help: Option<[String; 2]>,
    abbrev: bool,
}

impl<'a> Flags<'a> {
    fn new(prefixes: &'a str, add_help: bool, abbrev: bool) -> Self {
        let p = if prefixes.contains('-') {
            '-'
        } else {
            prefixes.chars().next().unwrap()
        };
        Self {
            prefixes,
            help: add_help.then(|| [format!("{p}h"), format!("{p}{p}help")]),
            abbrev,
        }
    }

    fn prefixed(&self, flag: &str) -> bool {
        flag.chars()
            .next()
            .is_some_and(|c| self.prefixes.contains(c))
    }
}

/// A class constant must resolve to the builtin in the builder's lexical scope.
fn builtin_constant<'a>(
    value: &'a ast::Expr,
    module: &SourceModule,
    locals: &HashSet<String>,
) -> Option<&'a str> {
    let name = value.as_name_expr()?.id.as_str();
    (annotations::is_builtin_type(name)
        && !module.bindings.contains_key(name)
        && !locals.contains(name))
    .then_some(name)
}

fn option(
    call: &ast::ExprCall,
    graph: &SourceGraph,
    id: ModuleId,
    types: &Types,
    classes: &Classes,
    flags: &Flags,
    locals: &HashSet<String>,
) -> Result<OptionField, String> {
    let module = &graph.modules[id];
    let Some((flag, rest)) = call.arguments.args.split_first() else {
        return Err(module.error(
            call,
            "add_argument requires a literal option or positional name",
        ));
    };
    let flag_name = literal(flag, module)?;
    let positional = !flags.prefixed(flag_name);
    let aliases = rest
        .iter()
        .map(|expr| literal(expr, module))
        .collect::<Result<Vec<_>, _>>()?;
    let kw = keywords(
        call,
        &[
            "type", "nargs", "help", "action", "default", "const", "required", "choices", "dest",
        ],
        module,
    )?;
    if positional && (!aliases.is_empty() || kw.contains_key("dest")) {
        return Err(module.error(flag, "argparse positional arguments have one destination"));
    }
    let spellings = std::iter::once(flag_name).chain(aliases.iter().copied());
    let dest = if let Some(expr) = kw.get("dest") {
        literal(expr, module)?.to_owned()
    } else if positional {
        flag_name.to_owned()
    } else {
        let spelling = spellings
            .clone()
            .find(|name| {
                name.chars()
                    .nth(1)
                    .is_some_and(|c| flags.prefixes.contains(c))
            })
            .unwrap_or(flag_name);
        spelling
            .trim_start_matches(|c| flags.prefixes.contains(c))
            .replace('-', "_")
    };
    if !dest
        .as_bytes()
        .first()
        .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_')
        || !dest.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
    {
        return Err(module.error(
            flag,
            "argparse requires a representable namespace field name",
        ));
    }
    let mut seen = HashSet::new();
    if !positional
        && spellings.into_iter().any(|s| {
            !flags.prefixed(s)
                || !seen.insert(s)
                || flags
                    .help
                    .as_ref()
                    .is_some_and(|help| help.iter().any(|h| h == s))
        })
    {
        return Err(module.error(
            flag,
            "argparse option spellings require distinct flags with a declared prefix",
        ));
    }
    if positional && kw.contains_key("required") {
        return Err(module.error(call, "argparse positionals do not accept required"));
    }
    let mut element = Type::Str;
    if let Some(&ty) = kw.get("type") {
        element = match graph.resolve(id, ty) {
            Some(Binding::Intrinsic(Intrinsic::Path)) => Type::Path,
            None if names(ty, "str") => Type::Str,
            None if names(ty, "int") => Type::Int,
            None if names(ty, "float") => Type::Float,
            _ => {
                return Err(
                    module.error(ty, "argparse type must be str, int, float or pathlib.Path")
                );
            }
        };
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
    let action = match kw.get("action").copied() {
        None => "store",
        Some(expr) if registration::boolean_action(expr, graph, id, locals) => "boolean_optional",
        Some(expr) => {
            let action = literal(expr, module)?;
            if action == "boolean_optional" {
                return Err(module.error(expr, "argparse BooleanOptionalAction must be the class"));
            }
            action
        }
    };
    // BooleanOptionalAction inserts the negative spelling immediately after
    // each long option; short aliases keep their original position.
    let aliases = if action == "boolean_optional" {
        let mut expanded = Vec::new();
        for spelling in std::iter::once(flag_name).chain(aliases) {
            if spelling.starts_with("--no-") {
                return Err(module.error(
                    flag,
                    &format!("invalid argparse option {spelling:?} for BooleanOptionalAction"),
                ));
            }
            expanded.push(spelling.to_owned());
            if let Some(name) = spelling.strip_prefix("--") {
                expanded.push(format!("--no-{name}"));
            }
        }
        expanded.into_iter().skip(1).collect()
    } else {
        aliases.into_iter().map(str::to_owned).collect::<Vec<_>>()
    };
    let zero = action != "store";
    if zero
        && (positional
            || kw.contains_key("type")
            || kw.contains_key("nargs")
            || kw.contains_key("choices")
            || action != "store_const" && kw.contains_key("const"))
    {
        return Err(module.error(call, "unsupported argparse action or action keywords"));
    }
    match action {
        "store" => {}
        "store_true" | "store_false" | "boolean_optional" => element = Type::Bool,
        "count" => {
            if let Some(value) = kw.get("default")
                && !matches!(value, ast::Expr::NoneLiteral(_))
                && numeric_literal(value, Type::Int).is_none()
            {
                return Err(
                    module.error(call, "argparse count requires an integer or None default")
                );
            }
            element = Type::Int;
        }
        "store_const" => {
            let invalid = || {
                module.error(
                    call,
                    "argparse store_const requires representable literal values",
                )
            };
            let kind = |key| {
                Ok(match kw.get(key).copied() {
                    None | Some(ast::Expr::NoneLiteral(_)) => None,
                    Some(ast::Expr::StringLiteral(_)) => Some(Type::Str),
                    Some(ast::Expr::BooleanLiteral(_)) => Some(Type::Bool),
                    Some(v) if builtin_constant(v, module, locals).is_some() => {
                        Some(Type::BuiltinClass)
                    }
                    Some(v) if numeric_literal(v, Type::Int).is_some() => Some(Type::Int),
                    Some(v) if numeric_literal(v, Type::Float).is_some() => Some(Type::Float),
                    _ => return Err(invalid()),
                })
            };
            element = match (kind("const")?, kind("default")?) {
                (None, None) => Type::Str,
                (Some(ty), None) | (None, Some(ty)) => ty,
                (Some(left), Some(right)) if left == right => left,
                // The string field below also supports integer constants and defaults.
                (Some(Type::Int), Some(Type::Str)) | (Some(Type::Str), Some(Type::Int)) => {
                    Type::Str
                }
                _ => return Err(invalid()),
            };
        }
        _ => return Err(module.error(call, "unsupported argparse action")),
    }
    // argparse converts strings but preserves an integer supplied as a default
    // or constant, even when the action would read a string from argv.
    let mixed_integer = |value: &ast::Expr| {
        !many && element == Type::Str && numeric_literal(value, Type::Int).is_some()
    };
    let stored_number = |value: &ast::Expr| {
        numeric_literal(
            value,
            if element == Type::Str {
                Type::Int
            } else {
                element
            },
        )
    };
    let int_default = kw.get("default").is_some_and(|v| mixed_integer(v));
    let int_constant = kw.get("const").is_some_and(|v| mixed_integer(v));
    let constant_number;
    let constant = match kw.get("const").copied() {
        None if action == "store_true" => Some("True"),
        None if action == "store_false" => Some("False"),
        None | Some(ast::Expr::NoneLiteral(_)) => None,
        Some(value) if element == Type::BuiltinClass => builtin_constant(value, module, locals),
        Some(ast::Expr::BooleanLiteral(value)) if action == "store_const" => {
            Some(if value.value { "True" } else { "False" })
        }
        Some(value)
            if (nargs == "optional" || action == "store_const")
                && value.is_string_literal_expr() =>
        {
            Some(literal(value, module)?)
        }
        Some(value) if nargs == "optional" || action == "store_const" => {
            constant_number = stored_number(value).ok_or_else(|| {
                module.error(
                    value,
                    "argparse constant requires a literal of the option type",
                )
            })?;
            Some(constant_number.as_str())
        }
        Some(value) => return Err(module.error(value, "argparse const requires nargs='?'")),
    };
    let mut range = None;
    let choices = if let Some(&choices) = kw.get("choices") {
        match choices {
            ast::Expr::ListComp(comp)
                if element == Type::Str
                    && let [clause] = comp.generators.as_slice()
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
                    .map(|(_, value)| value.to_string())
                    .collect()
            }
            ast::Expr::StringLiteral(_) if element == Type::Str => literal(choices, module)?
                .chars()
                .map(|ch| ch.to_string())
                .collect(),
            ast::Expr::Call(call_range)
                if element == Type::Int
                    && names(&call_range.func, "range")
                    && graph.resolve(id, &call_range.func).is_none() =>
            {
                range = Some(range_choices(call_range).ok_or_else(|| {
                    module.error(
                        choices,
                        "argparse choices require a literal integer range with nonzero step",
                    )
                })?);
                Vec::new()
            }
            items => {
                let items = match items {
                    ast::Expr::List(list) => list.elts.as_slice(),
                    ast::Expr::Tuple(tuple) => tuple.elts.as_slice(),
                    _ => return Err(module.error(choices, "argparse choices require literal values, a literal integer range, or the values of a sealed enum")),
                };
                items
                    .iter()
                    .map(|item| match element {
                        Type::Str => Ok(literal(item, module)?.to_owned()),
                        Type::Int | Type::Float => {
                            numeric_literal(item, element).ok_or_else(|| {
                                module.error(item, "argparse choices must match the option type")
                            })
                        }
                        _ => Err(module.error(item, "unsupported argparse choice type")),
                    })
                    .collect::<Result<Vec<_>, _>>()?
            }
        }
    } else {
        Vec::new()
    };
    let mut enum_default = None;
    let enum_default_text;
    let default_number;
    let default = match kw.get("default").copied() {
        None if action == "store_true" => Some("False"),
        None if action == "store_false" => Some("True"),
        None | Some(ast::Expr::NoneLiteral(_)) => None,
        Some(value) if element == Type::BuiltinClass => builtin_constant(value, module, locals),
        Some(expr @ ast::Expr::StringLiteral(_))
            if element == Type::Str
                || !many && matches!(element, Type::Path | Type::Int | Type::Float) =>
        {
            Some(literal(expr, module)?)
        }
        Some(value) if int_default || !many && matches!(element, Type::Int | Type::Float) => {
            default_number = stored_number(value).ok_or_else(|| {
                module.error(
                    value,
                    "argparse default requires a literal of the option type",
                )
            })?;
            Some(default_number.as_str())
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
    if enum_default.is_some() && int_constant {
        return Err(module.error(
            call,
            "argparse cannot represent mixed enum, integer and string values in one field",
        ));
    }
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
    let kind = match element {
        Type::Str => "str",
        Type::Int => "int",
        Type::Float => "float",
        Type::Path => "path",
        Type::Bool => "bool",
        Type::BuiltinClass => "class",
        _ => unreachable!(),
    };
    let converter = kw
        .get("type")
        .map(|_| if element == Type::Path { "Path" } else { kind });
    let numeric_kw = |name, ty| {
        kw.get(name)
            .is_some_and(|v| numeric_literal(v, ty).is_some())
    };
    let optional = (!required && default.is_none() && !(positional && nargs == "zero_or_more"))
        || ((!positional && nargs == "optional" || action == "store_const") && constant.is_none());
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
    } else if int_default || int_constant {
        types.string_or_int()
    } else if element == Type::Str && !choices.is_empty() {
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
        name: dest.clone(),
        annotation: flag.clone(),
        ty: if optional { types.optional(ty) } else { ty },
        option: json!({ "flag": flag_name, "dest": dest, "positional": positional, "aliases": aliases, "action": action, "enum_default": enum_default.map(|(_, tag)| tag), "kind": kind, "nargs": nargs, "constant": constant, "list_default":list_default, "required": required,
            "choices": choices, "range":range, "has_choices": kw.contains_key("choices"), "int_default": int_default || element == Type::Int && numeric_kw("default", element),
            "int_constant": int_constant || element == Type::Int && numeric_kw("const", element),
            "float_default": element == Type::Float && numeric_kw("default", element),
            "float_constant": element == Type::Float && numeric_kw("const", element), "converter": converter, "default": default, "path_default": path_default, "help": help, "optional": optional }),
    })
}

#[derive(Default)]
struct RegisteredArguments {
    options: Vec<OptionField>,
    groups: Vec<Value>,
    mutex_required: Vec<bool>,
}

struct Subcommand {
    name: String,
    aliases: Vec<String>,
    help: Option<String>,
    arguments: RegisteredArguments,
}

impl Subcommand {
    fn new(
        call: &ast::ExprCall,
        module: &SourceModule,
        spellings: &mut HashSet<String>,
    ) -> Result<Self, String> {
        let kw = keywords(call, &["name", "help", "aliases"], module)?;
        let name = match (call.arguments.args.as_ref(), kw.get("name").copied()) {
            ([name], None) | ([], Some(name)) => name,
            _ => {
                return Err(module.error(call, "add_parser requires one literal subcommand name"));
            }
        };
        let name = literal(name, module)?;
        let aliases = match kw.get("aliases").copied() {
            None => &[][..],
            Some(ast::Expr::List(list)) => list.elts.as_slice(),
            Some(ast::Expr::Tuple(tuple)) => tuple.elts.as_slice(),
            Some(expr) => {
                return Err(
                    module.error(expr, "subcommand aliases require a literal list or tuple")
                );
            }
        }
        .iter()
        .map(|a| literal(a, module))
        .collect::<Result<Vec<_>, _>>()?;
        let names = std::iter::once(name).chain(aliases.iter().copied());
        if names.clone().any(|word| spellings.contains(word)) {
            return Err(module.error(
                call,
                "subcommand name or alias conflicts with a previously registered parser",
            ));
        }
        // CPython checks aliases against earlier parsers before
        // registering any spelling of this child.
        spellings.extend(names.map(str::to_owned));
        let help = kw
            .get("help")
            .map(|value| literal(value, module))
            .transpose()?;
        if help.is_some_and(|help| !template(help, &["prog"])) {
            return Err(module.error(call, "unsupported argparse subcommand help substitution"));
        }
        Ok(Self {
            name: name.into(),
            aliases: aliases.into_iter().map(str::to_owned).collect(),
            help: help.map(str::to_owned),
            arguments: RegisteredArguments::default(),
        })
    }
}

fn builder_call<'a>(
    statement: Option<&'a ast::Stmt>,
    receiver: &str,
    operation: &str,
) -> Option<(Option<&'a ast::ExprName>, &'a ast::ExprCall)> {
    if let Some((child, call)) = assigned_call(statement, receiver, operation) {
        return Some((Some(child), call));
    }
    let ast::Stmt::Expr(stmt) = statement? else {
        return None;
    };
    Some((None, method(&stmt.value, receiver, operation)?))
}

fn required_assignment<'a>(statement: Option<&'a ast::Stmt>, group: &str) -> Option<&'a ast::Expr> {
    let ast::Stmt::Assign(stmt) = statement? else {
        return None;
    };
    let [ast::Expr::Attribute(attr)] = stmt.targets.as_slice() else {
        return None;
    };
    (names(&attr.value, group) && attr.attr.as_str() == "required").then_some(&stmt.value)
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
        forwarder: None,
        init_hooks: Default::default(),
        init_exposes_self: false,
        exception_aliases: Default::default(),
        static_bindings: Default::default(),
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
        mapping: false,
        dataclass: Some(DataclassOptions {
            value_struct: false,
            slots: false,
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
    let (name, owner, call) = assigned_method(statement?, operation)?;
    (owner == receiver).then_some((name, call))
}

fn assigned_method<'a>(
    statement: &'a ast::Stmt,
    operation: &str,
) -> Option<(&'a ast::ExprName, &'a str, &'a ast::ExprCall)> {
    let ast::Stmt::Assign(assign) = statement else {
        return None;
    };
    let [ast::Expr::Name(name)] = assign.targets.as_slice() else {
        return None;
    };
    let (owner, call) = method_receiver(&assign.value, operation)?;
    Some((name, owner, call))
}

/// A literal builder may be read repeatedly through its parser operations. Any
/// other use, including passing the builder as an argument, would escape it.
struct Uses<'a> {
    builder: &'a str,
    operations: Vec<TextSize>,
    registration_errors: &'a HashMap<TextSize, Expr>,
    escaped: bool,
    allow_calls: bool,
}

impl<'ast> Visitor<'ast> for Uses<'_> {
    fn visit_expr(&mut self, node: &'ast ast::Expr) {
        if self.allow_calls
            && let ast::Expr::Call(call) = node
            && let ast::Expr::Attribute(attr) = &*call.func
            && names(&attr.value, self.builder)
            && matches!(
                attr.attr.as_str(),
                "parse_args"
                    | "parse_known_args"
                    | "format_help"
                    | "format_usage"
                    | "print_help"
                    | "print_usage"
            )
        {
            self.operations.push(call.start());
            self.visit_arguments(&call.arguments);
        } else if let ast::Expr::Call(call) = node
            && self.registration_errors.contains_key(&call.start())
        {
            self.visit_arguments(&call.arguments);
        } else {
            self.escaped |= names(node, self.builder);
            walk_expr(self, node);
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
            .is_some_and(|commands| commands.by_name.values().any(|&value| value == child))
    }

    /// Guards may specialize a value for one command. A shared handler or
    /// assignment can widen siblings to exactly their common record prefix.
    pub(super) fn common_namespace(&self, left: Type, right: Type) -> Option<Type> {
        let (Type::Class(left), Type::Class(right)) = (left, right) else {
            return None;
        };
        self.commands.iter().find_map(|(&parent, commands)| {
            let belongs =
                |class| class == parent || commands.by_name.values().any(|&child| child == class);
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
                    let kw = keywords(
                        ctor,
                        &[
                            "description",
                            "formatter_class",
                            "prog",
                            "prefix_chars",
                            "add_help",
                            "allow_abbrev",
                        ],
                        module,
                    )?;
                    let prefixes = kw
                        .get("prefix_chars")
                        .map(|e| literal(e, module))
                        .transpose()?
                        .unwrap_or("-");
                    if prefixes.is_empty() {
                        return Err(
                            module.error(ctor, "argparse requires at least one prefix character")
                        );
                    }
                    let boolean = |name| match kw.get(name) {
                        None => Ok(true),
                        Some(ast::Expr::BooleanLiteral(value)) => Ok(value.value),
                        _ => Err(module.error(ctor, "argparse flags require literal booleans")),
                    };
                    let flags =
                        Flags::new(prefixes, boolean("add_help")?, boolean("allow_abbrev")?);
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
                    let mut locals = crate::narrowing::assigned_names(&function.body);
                    locals.extend(
                        function
                            .parameters
                            .iter_non_variadic_params()
                            .map(|p| p.name().to_string()),
                    );
                    let mut registration_errors = HashMap::new();
                    let mut read_options =
                        |builder: &str,
                         flags: &Flags,
                         index: &mut usize,
                         mut names: HashSet<String>,
                         builders: &mut Vec<String>|
                         -> Result<RegisteredArguments, String> {
                            let failures = |owner, from| {
                                registration::failures(
                                    &function.body[from..],
                                    owner,
                                    graph,
                                    module_id,
                                    &locals,
                                    flags.prefixes,
                                )
                            };
                            registration_errors.extend(failures(builder, *index));
                            let mut args = RegisteredArguments::default();
                            let mut spellings = HashSet::new();
                            let mut group_names = HashMap::new();
                            let mut mutex_names = HashMap::new();
                            while let Some(stmt) = function.body.get(*index) {
                                if let Some((group, call)) =
                                    assigned_call(Some(stmt), builder, "add_argument_group")
                                {
                                    register(group, builders)?;
                                    builders.push(group.id.to_string());
                                    registration_errors
                                        .extend(failures(group.id.as_str(), *index + 1));
                                    group_names.insert(group.id.as_str(), args.groups.len());
                                    args.groups.push(argument_group(call, module)?);
                                    *index += 1;
                                    continue;
                                }
                                if let Some((group, owner, call)) =
                                    assigned_method(stmt, "add_mutually_exclusive_group")
                                    && (owner == builder || group_names.contains_key(owner))
                                {
                                    register(group, builders)?;
                                    let kw = keywords(call, &["required"], module)?;
                                    if !call.arguments.args.is_empty() {
                                        return Err(module.error(
                                            call,
                                            "mutually exclusive groups require named arguments",
                                        ));
                                    }
                                    let required = match kw.get("required") {
                                        None | Some(ast::Expr::NoneLiteral(_)) => false,
                                        Some(ast::Expr::BooleanLiteral(value)) => value.value,
                                        Some(expr) => {
                                            return Err(module.error(
                                                *expr,
                                                "group required must be a literal bool or None",
                                            ));
                                        }
                                    };
                                    builders.push(group.id.to_string());
                                    registration_errors
                                        .extend(failures(group.id.as_str(), *index + 1));
                                    mutex_names.insert(
                                        group.id.as_str(),
                                        (
                                            args.mutex_required.len(),
                                            group_names.get(owner).copied(),
                                        ),
                                    );
                                    args.mutex_required.push(required);
                                    *index += 1;
                                    continue;
                                }
                                let ast::Stmt::Expr(stmt) = stmt else { break };
                                let Some((owner, add)) =
                                    method_receiver(&stmt.value, "add_argument")
                                else {
                                    break;
                                };
                                let mutex = mutex_names.get(owner).copied();
                                let group = group_names.get(owner).copied().or_else(|| mutex?.1);
                                if owner != builder && group.is_none() && mutex.is_none() {
                                    break;
                                }
                                if registration_errors.contains_key(&add.start()) {
                                    *index += 1;
                                    continue;
                                }
                                let mut field =
                                    option(add, graph, module_id, types, classes, flags, &locals)?;
                                if let Some(group) = group {
                                    field.option["group"] = json!(group);
                                }
                                if let Some((id, _)) = mutex {
                                    if field.option["required"] == true {
                                        return Err(module.error(
                                            add,
                                            "mutually exclusive arguments must be optional",
                                        ));
                                    }
                                    field.option["mutex"] = json!(id);
                                }
                                for value in std::iter::once(&field.option["flag"])
                                    .chain(field.option["aliases"].as_array().unwrap())
                                {
                                    let spelling = value.as_str().unwrap();
                                    if !spellings.insert(spelling.to_owned()) {
                                        return Err(module.error(
                                            add,
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
                                args.options.push(field);
                                *index += 1;
                            }
                            Ok(args)
                        };
                    let mut index = start + 1;
                    let RegisteredArguments {
                        options,
                        groups,
                        mutex_required,
                    } = read_options(builder, &flags, &mut index, HashSet::new(), &mut builders)?;
                    let mut root_names = options
                        .iter()
                        .map(|field| field.name.clone())
                        .collect::<HashSet<_>>();
                    let mut children = Vec::new();
                    let mut command = None;
                    let mut command_config = None;
                    if let Some((group, add)) =
                        builder_call(function.body.get(index), builder, "add_subparsers")
                    {
                        if let Some(group) = group {
                            register(group, &builders)?;
                            builders.push(group.id.to_string());
                        }
                        let kw = keywords(
                            add,
                            &[
                                "dest",
                                "required",
                                "metavar",
                                "help",
                                "title",
                                "description",
                            ],
                            module,
                        )?;
                        if !add.arguments.args.is_empty() {
                            return Err(
                                module.error(add, "add_subparsers accepts keyword arguments")
                            );
                        }
                        let text =
                            |name| kw.get(name).map(|value| literal(value, module)).transpose();
                        let dest = text("dest")?;
                        if let Some(dest) = dest
                            && (!dest
                                .as_bytes()
                                .first()
                                .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_')
                                || !dest.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
                                || !root_names.insert(dest.to_owned()))
                        {
                            return Err(module.error(
                                add,
                                "subparser destination must be a distinct namespace field",
                            ));
                        }
                        let boolean = |expr: &ast::Expr| match expr {
                            ast::Expr::BooleanLiteral(value) => Ok(value.value),
                            _ => {
                                Err(module.error(expr, "subparser required must be a literal bool"))
                            }
                        };
                        let mut required = kw
                            .get("required")
                            .map(|v| boolean(v))
                            .transpose()?
                            .unwrap_or(false);
                        let metavar = text("metavar")?;
                        let help = text("help")?;
                        if help.is_some_and(|help| !template(help, &["prog"])) {
                            return Err(module
                                .error(add, "unsupported argparse subcommand help substitution"));
                        }
                        let section = if kw.contains_key("title") || kw.contains_key("description")
                        {
                            let text = |key, default| match kw.get(key).copied() {
                                None => Ok(default),
                                Some(ast::Expr::NoneLiteral(_)) => Ok(None),
                                Some(expr) => literal(expr, module).map(Some),
                            };
                            let title = text("title", Some("subcommands"))?;
                            let description = text("description", None)?;
                            if description
                                .is_some_and(|s| s.contains("%(prog)") && !template(s, &["prog"]))
                            {
                                return Err(module.error(
                                    add,
                                    "unsupported argparse group description substitution",
                                ));
                            }
                            Some(json!({"title": title, "description": description}))
                        } else {
                            None
                        };
                        index += 1;
                        let mut spellings = HashSet::new();
                        if let Some(group) = group {
                            loop {
                                if let Some(value) =
                                    required_assignment(function.body.get(index), group.id.as_str())
                                {
                                    required = boolean(value)?;
                                    index += 1;
                                    continue;
                                }
                                let Some((child, call)) = builder_call(
                                    function.body.get(index),
                                    group.id.as_str(),
                                    "add_parser",
                                ) else {
                                    break;
                                };
                                if let Some(child) = child {
                                    register(child, &builders)?;
                                    builders.push(child.id.to_string());
                                }
                                let mut subcommand = Subcommand::new(call, module, &mut spellings)?;
                                index += 1;
                                if let Some(child) = child {
                                    subcommand.arguments = read_options(
                                        child.id.as_str(),
                                        &Flags::new("-", true, true),
                                        &mut index,
                                        root_names.clone(),
                                        &mut builders,
                                    )?;
                                }
                                children.push(subcommand);
                            }
                        }
                        command = dest.map(|dest| {
                            let names = if spellings.is_empty() {
                                Type::Str
                            } else {
                                types.string_literal(spellings.into_iter().collect())
                            };
                            OptionField {
                                name: dest.to_owned(),
                                annotation: (*add.func).clone(),
                                ty: if required {
                                    names
                                } else {
                                    types.optional(names)
                                },
                                option: Value::Null,
                            }
                        });
                        command_config = Some(
                            json!({"dest": dest, "required": required, "metavar": metavar, "help": help, "group": section}),
                        );
                    }
                    let mut operations = Vec::new();
                    for declared in &builders {
                        let mut uses = Uses {
                            builder: declared,
                            operations: Vec::new(),
                            registration_errors: &registration_errors,
                            escaped: false,
                            allow_calls: false,
                        };
                        uses.visit_body(&function.body[..start]);
                        uses.allow_calls = declared == builder;
                        uses.visit_body(&function.body[index..]);
                        if uses.escaped {
                            return Err(module.error(
                                name,
                                "argparse builder cannot escape its declaration or be rebound",
                            ));
                        }
                        operations.extend(uses.operations);
                    }
                    if operations.is_empty() && registration_errors.is_empty() {
                        return Err(module.error(
                            ctor,
                            "argparse declarations must be followed by a parser operation",
                        ));
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
                            .chain(&child.arguments.options)
                            .collect::<Vec<_>>();
                        let variant =
                            namespace_class(classes, layouts, definition, ctor.range(), &fields);
                        parsers.namespaces.insert(variant);
                        child_classes.push(variant);
                        tags.insert(child.name.clone(), variant);
                        tags.extend(child.aliases.iter().map(|alias| (alias.clone(), variant)));
                        child_schemas.push(json!({
                            "name": child.name, "aliases": child.aliases, "help": child.help,
                            "raw": false, "prefixes": "-", "help_flags":["-h","--help"], "abbrev":true,
                            "options": child.arguments.options.iter().map(|f| &f.option).collect::<Vec<_>>(),
                            "groups": child.arguments.groups, "mutex_required": child.arguments.mutex_required,
                        }));
                    }
                    let subcommands = command_config.map(|mut config| {
                        let dest = command.as_ref().map(|field| field.name.clone());
                        parsers.commands.insert(
                            class,
                            CommandVariants {
                                destination: dest,
                                by_name: tags,
                            },
                        );
                        config["choices"] = Value::Array(child_schemas);
                        config
                    });
                    for stmt in &function.body[start..index] {
                        if !matches!(stmt, ast::Stmt::Expr(stmt)
                            if registration_errors.contains_key(&stmt.value.start()))
                        {
                            parsers.declarations.insert((module_id, stmt.start()));
                        }
                    }
                    parsers.registration_errors.extend(
                        registration_errors
                            .into_iter()
                            .map(|(pos, error)| ((module_id, pos), error)),
                    );
                    let options = options
                        .into_iter()
                        .map(|field| field.option)
                        .collect::<Vec<_>>();
                    let schema = json!({"prog": prog, "raw": raw, "prefixes": flags.prefixes, "help_flags": flags.help, "abbrev": flags.abbrev, "options": options, "groups": groups, "mutex_required": mutex_required, "subcommands": subcommands}).to_string();
                    let parser = Parser {
                        class,
                        children: child_classes,
                        schema,
                        description: description.cloned(),
                    };
                    for operation in operations {
                        parsers.calls.insert((module_id, operation), parser.clone());
                    }
                }
            }
        }
        Ok(parsers)
    }
}

impl Checker<'_> {
    pub(super) fn parse_args(&mut self, call: &ast::ExprCall) -> Result<Option<Expr>, String> {
        if let Some(error) = self
            .classes
            .parsers
            .registration_errors
            .get(&(self.module, call.start()))
        {
            return registration::at_call(self, call, error.clone()).map(Some);
        }
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
        let schema = Expr {
            ty: Type::Str,
            kind: ExprKind::Str(parser.schema.clone()),
        };
        let method = call.func.as_attribute_expr().unwrap().attr.as_str();
        if !matches!(method, "parse_args" | "parse_known_args") {
            let print = method.starts_with("print");
            let file = match (
                call.arguments.args.as_ref(),
                call.arguments.keywords.as_ref(),
            ) {
                ([], []) => None,
                ([file], []) if print => Some(file),
                ([], [kw]) if print && kw.arg.as_ref().is_some_and(|name| name == "file") => {
                    Some(&kw.value)
                }
                _ => {
                    return Err(error(
                        self.source,
                        call,
                        "invalid argparse formatting arguments",
                    ));
                }
            };
            let mut args = vec![schema, description];
            let output = match file {
                _ if !print => ir::ArgparseOutput::Return,
                None | Some(ast::Expr::NoneLiteral(_)) => ir::ArgparseOutput::Stdout,
                Some(file) => match self.resolve(file) {
                    Some(Binding::Intrinsic(Intrinsic::Stdout)) => ir::ArgparseOutput::Stdout,
                    Some(Binding::Intrinsic(Intrinsic::Stderr)) => ir::ArgparseOutput::Stderr,
                    _ => {
                        args.push(self.expr_expected(file, Type::TextIO)?);
                        ir::ArgparseOutput::File
                    }
                },
            };
            return Ok(Some(Expr {
                ty: if print { Type::None } else { Type::Str },
                kind: ExprKind::Builtin(
                    Builtin::ArgparseText {
                        usage: method.ends_with("usage"),
                        output,
                    },
                    args,
                ),
            }));
        }
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
                    &format!("argparse {method} accepts one argument list"),
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
        let namespace = Type::Class(parser.class);
        Ok(Some(Expr {
            ty: if method == "parse_known_args" {
                self.types.tuple(vec![namespace, argv_type])
            } else {
                namespace
            },
            kind: ExprKind::Builtin(
                Builtin::ParseArgs {
                    namespace: parser.class,
                    children: parser.children.clone(),
                    process_argv: explicit.is_none(),
                },
                vec![schema, description, argv],
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
        if let Some(commands) = self.classes.parsers.commands.get(&class)
            && commands.destination.as_deref() == Some(attribute.attr.as_str())
            && let Some(&variant) = commands.by_name.get(value)
        {
            self.narrowed.insert_mut(slot, Type::Class(variant));
        }
    }

    pub(super) fn enter_namespace_guards(
        &mut self,
        test: &ast::Expr,
        truth: bool,
        body: &[ast::Stmt],
    ) {
        if let ast::Expr::BoolOp(value) = test
            && (value.op == ast::BoolOp::And && truth || value.op == ast::BoolOp::Or && !truth)
        {
            // A later operand may rebind the receiver even if the body does not.
            let mut reassigned = false;
            for operand in value.values.iter().rev() {
                if !reassigned && let Some(guard) = self.namespace_guard(operand, truth, body) {
                    self.namespace_guards.push(guard);
                }
                if !matches!(operand, ast::Expr::Attribute(attr) if attr.value.is_name_expr()) {
                    reassigned = true
                }
            }
        } else if let Some(guard) = self.namespace_guard(test, truth, body) {
            self.namespace_guards.push(guard);
        }
    }

    /// An immutable optional field stays present in this arm while its local
    /// receiver keeps the same binding, even when another alias survives calls.
    pub(super) fn namespace_guard(
        &self,
        mut test: &ast::Expr,
        mut truth: bool,
        body: &[ast::Stmt],
    ) -> Option<(usize, usize)> {
        while let ast::Expr::UnaryOp(unary) = test
            && unary.op == ast::UnaryOp::Not
        {
            truth = !truth;
            test = &unary.operand;
        }
        let attr = match test {
            ast::Expr::Attribute(attr) if truth => attr,
            ast::Expr::Compare(compare) => {
                let (operand, present) = narrowing::none_identity(compare, truth)?;
                if !present {
                    return None;
                }
                operand.as_attribute_expr()?
            }
            _ => return None,
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
