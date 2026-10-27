//! Python AST nodes and traversal for runtime-loaded modules.
use std::collections::{HashMap, VecDeque};

use super::super::{Context, Error, Id, NONE, Result, Value, builtins};

mod parse;

#[derive(Clone, Copy)]
enum DefaultField {
    Required,
    Optional,
    List,
    Load,
}

use DefaultField::{List, Load, Optional, Required};

struct Node {
    name: &'static str,
    base: &'static str,
    fields: &'static [(&'static str, DefaultField)],
    positions: bool,
}

macro_rules! node {
    ($name:ident : $base:ident, $positions:expr; $($field:ident : $default:ident),*) => {
        Node { name: stringify!($name), base: stringify!($base), positions: $positions,
               fields: &[$((stringify!($field), $default)),*] }
    };
}

const NODES: &[Node] = &[
    node!(AST: object, false;),
    node!(mod: AST, false;),
    node!(stmt: AST, true;),
    node!(expr: AST, true;),
    node!(expr_context: AST, false;),
    node!(Module: mod, false; body: List, type_ignores: List),
    node!(Expression: mod, false; body: Required),
    node!(FunctionDef: stmt, true; name: Required, args: Required, body: List, decorator_list: List, returns: Optional, type_comment: Optional, type_params: List),
    node!(arguments: AST, false; posonlyargs: List, args: List, vararg: Optional, kwonlyargs: List, kw_defaults: List, kwarg: Optional, defaults: List),
    node!(arg: AST, true; arg: Required, annotation: Optional, type_comment: Optional),
    node!(Pass: stmt, true;),
    node!(Expr: stmt, true; value: Required),
    node!(If: stmt, true; test: Required, body: List, orelse: List),
    node!(Assign: stmt, true; targets: List, value: Required, type_comment: Optional),
    node!(Return: stmt, true; value: Optional),
    node!(Name: expr, true; id: Required, ctx: Load),
    node!(Constant: expr, true; value: Required, kind: Optional),
    node!(Attribute: expr, true; value: Required, attr: Required, ctx: Load),
    node!(Subscript: expr, true; value: Required, slice: Required, ctx: Load),
    node!(Call: expr, true; func: Required, args: List, keywords: List),
    node!(keyword: AST, true; arg: Optional, value: Required),
    node!(IfExp: expr, true; test: Required, body: Required, orelse: Required),
    node!(Tuple: expr, true; elts: List, ctx: Load),
    node!(List: expr, true; elts: List, ctx: Load),
    node!(JoinedStr: expr, true; values: List),
    node!(FormattedValue: expr, true; value: Required, conversion: Required, format_spec: Optional),
    node!(Load: expr_context, false;),
    node!(Store: expr_context, false;),
];

const POSITIONS: [&str; 4] = ["lineno", "col_offset", "end_lineno", "end_col_offset"];

fn string_tuple<'a>(ctx: &mut Context, values: impl IntoIterator<Item = &'a str>) -> Id {
    let values = values.into_iter().map(|value| ctx.text(value)).collect();
    ctx.alloc(Value::Tuple(values))
}

pub(in super::super) fn install(ctx: &mut Context, module: Id) {
    let object = ctx.builtin("object");
    let module_name = ctx.text("ast");
    let positions = string_tuple(ctx, POSITIONS);
    let empty = ctx.alloc(Value::Tuple(Vec::new()));
    for spec in NODES {
        let scope = ctx.scope("ast", Some(module));
        ctx.bind(scope, "__module__", module_name);
        let fields = string_tuple(ctx, spec.fields.iter().map(|(key, _)| *key));
        ctx.bind(scope, "_fields", fields);
        ctx.bind(
            scope,
            "_attributes",
            if spec.positions { positions } else { empty },
        );
        if spec.positions {
            ctx.bind(scope, "end_lineno", NONE);
            ctx.bind(scope, "end_col_offset", NONE);
        }
        if spec.name == "AST" {
            let init = ctx.builtin("ast.AST.__init__");
            ctx.bind(scope, "__init__", init);
        }
        let base = if spec.base == "object" {
            object
        } else {
            ctx.scope_data(module).names[spec.base]
        };
        let ancestors = ctx.lineage(base);
        let class = ctx.alloc(Value::Class {
            namedtuple_fields: None,
            name: spec.name.into(),
            scope,
            ancestors,
        });
        ctx.builtins.insert(format!("ast.{}", spec.name), class);
        ctx.bind(module, spec.name, class);
    }
    for kind in ["Load", "Store"] {
        let singleton = parsed_instance(ctx, kind, &[], None);
        ctx.builtins.insert(format!("ast._{kind}"), singleton);
    }
    for name in ["parse", "walk"] {
        let callable = ctx.builtin(&format!("ast.{name}"));
        ctx.bind(module, name, callable);
    }
}

fn init(ctx: &mut Context, instance: Id, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let Value::Instance { .. } = ctx.heap.get(instance) else {
        return Err(Error::new(
            "TypeError",
            "AST constructor requires an AST instance",
        ));
    };
    let spec = NODES
        .iter()
        .rev()
        .find(|s| ctx.instance_of(instance, ctx.builtins[&format!("ast.{}", s.name)]))
        .ok_or_else(|| Error::new("TypeError", "expected AST instance"))?;
    if args.len() > spec.fields.len() {
        let count = spec.fields.len();
        return Err(Error::new(
            "TypeError",
            format!(
                "{} constructor takes at most {count} positional argument{}",
                spec.name,
                if count == 1 { "" } else { "s" }
            ),
        ));
    }
    let mut fields: HashMap<String, Id> = args
        .iter()
        .zip(spec.fields)
        .map(|(&v, (k, _))| ((*k).to_owned(), v))
        .collect();
    for (key, value) in kwargs {
        if fields.insert(key.clone(), *value).is_some() {
            return Err(Error::new(
                "TypeError",
                format!("{} got multiple values for argument '{key}'", spec.name),
            ));
        }
    }
    for &(name, default) in spec.fields {
        if !fields.contains_key(name) {
            let value = match default {
                Required => continue,
                Optional => NONE,
                List => ctx.alloc(Value::List(Vec::new())),
                Load => ctx.builtins["ast._Load"],
            };
            fields.insert(name.into(), value);
        }
    }
    let Value::Instance {
        fields: existing, ..
    } = ctx.heap.get_mut(instance)
    else {
        unreachable!()
    };
    existing.extend(fields);
    Ok(NONE)
}

// The parser constructs the canonical AST objects without invoking mutable Python constructors.
fn parsed_instance(
    ctx: &mut Context,
    kind: &str,
    values: &[Id],
    positions: Option<[i64; 4]>,
) -> Id {
    let spec = NODES
        .iter()
        .find(|s| s.name == kind)
        .expect("known AST node");
    debug_assert_eq!(values.len(), spec.fields.len());
    let mut fields: HashMap<_, _> = spec
        .fields
        .iter()
        .zip(values)
        .map(|((name, _), val)| ((*name).into(), *val))
        .collect();
    if let Some(pos) = positions {
        fields.extend(
            POSITIONS
                .into_iter()
                .zip(pos)
                .map(|(k, n)| (k.into(), ctx.int(n))),
        );
    }
    ctx.alloc(Value::Instance {
        class: ctx.builtins[&format!("ast.{kind}")],
        fields,
        dictionary: None,
        scalar: None,
        memory_stream: None,
    })
}

pub(in super::super) fn has_item(ctx: &Context, iter: Id) -> Result<bool> {
    let Value::AstWalk { queue, running } = ctx.heap.get(iter) else {
        unreachable!()
    };
    if *running {
        return Err(Error::new("ValueError", "generator already executing"));
    }
    Ok(!queue.is_empty())
}

pub(in super::super) fn advance(ctx: &mut Context, iter: Id) -> Result<Option<Id>> {
    if !has_item(ctx, iter)? {
        return Ok(None);
    }
    let Value::AstWalk { queue, running } = ctx.heap.get_mut(iter) else {
        unreachable!()
    };
    let node = queue.pop_front().expect("checked nonempty AST walk");
    *running = true;
    let result = children(ctx, node);
    let Value::AstWalk { queue, running } = ctx.heap.get_mut(iter) else {
        unreachable!()
    };
    *running = false;
    match result {
        Ok(children) => {
            queue.extend(children);
            Ok(Some(node))
        }
        Err(error) => {
            queue.clear();
            Err(error)
        }
    }
}

fn children(ctx: &mut Context, node: Id) -> Result<Vec<Id>> {
    ctx.heap.keep(node);
    let fields = ctx.lookup_attr(node, "_fields")?;
    let fields = ctx.iterator(fields)?;
    ctx.heap.keep(fields);
    let mut children = Vec::new();
    let ast = ctx.builtins["ast.AST"];
    while let Some(field) = ctx.next_item(fields)? {
        let name = ctx.str_value(field)?.to_owned();
        let value = match ctx.lookup_attr(node, &name) {
            Ok(value) => value,
            Err(error) if error.kind == "AttributeError" => continue,
            Err(error) => return Err(error),
        };
        let first = children.len();
        if ctx.instance_of(value, ast) {
            children.push(value);
        } else if let Value::List(values) = ctx.heap.get(value) {
            children.extend(values.iter().copied().filter(|&v| ctx.instance_of(v, ast)));
        }
        // A later field getter can detach children before they reach the walker's queue.
        for &child in &children[first..] {
            ctx.heap.keep(child);
        }
    }
    Ok(children)
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    match name {
        "AST.__init__" => {
            let Some((&instance, args)) = args.split_first() else {
                return Err(Error::new("TypeError", "AST.__init__ requires an instance"));
            };
            init(ctx, instance, args, kwargs)
        }
        "walk" => {
            let [node] = builtins::bind("walk", args, kwargs, ["node"])?;
            let node = node.ok_or_else(|| {
                Error::new(
                    "TypeError",
                    "walk() missing 1 required positional argument: 'node'",
                )
            })?;
            Ok(ctx.alloc(Value::AstWalk {
                queue: VecDeque::from([node]),
                running: false,
            }))
        }
        "parse" => parse::parse(ctx, args, kwargs),
        _ => Err(Error::new(
            "NotImplementedError",
            format!("unsupported AST operation {name}"),
        )),
    }
}
