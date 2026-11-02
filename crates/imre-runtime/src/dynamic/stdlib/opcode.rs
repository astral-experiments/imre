//! Introspection of the CPython 3.14 instruction set from native Python code.

use super::{Context, Error, FALSE, Id, NONE, Result, TRUE, Value};

mod metadata;

pub(in crate::dynamic) fn signature(name: &str) -> &'static str {
    match name {
        "stack_effect" => "($module, opcode, oparg=None, /, *, jump=None)",
        "get_executor" => "($module, /, code, offset)",
        "is_valid" | "has_arg" | "has_const" | "has_name" | "has_jump" | "has_free"
        | "has_local" | "has_exc" => "($module, /, opcode)",
        _ => "($module, /)",
    }
}

const FUNCTIONS: &[(&str, &str)] = &[
    ("stack_effect", "Compute the stack effect of the opcode."),
    (
        "is_valid",
        "Return True if opcode is valid, False otherwise.",
    ),
    (
        "has_arg",
        "Return True if the opcode uses its oparg, False otherwise.",
    ),
    (
        "has_const",
        "Return True if the opcode accesses a constant, False otherwise.",
    ),
    (
        "has_name",
        "Return True if the opcode accesses an attribute by name, False otherwise.",
    ),
    (
        "has_jump",
        "Return True if the opcode has a jump target, False otherwise.",
    ),
    (
        "has_free",
        concat!(
            "Return True if the opcode accesses a free variable, False otherwise.\n",
            "\n",
            "Note that 'free' in this context refers to names in the current scope\n",
            "that are referenced by inner scopes or names in outer scopes that are\n",
            "referenced from this scope. It does not include references to global\n",
            "or builtin scopes.",
        ),
    ),
    (
        "has_local",
        "Return True if the opcode accesses a local variable, False otherwise.",
    ),
    (
        "has_exc",
        "Return True if the opcode sets an exception handler, False otherwise.",
    ),
    (
        "get_specialization_stats",
        "Return the specialization stats",
    ),
    (
        "get_nb_ops",
        "Return array of symbols of binary ops.\n\nIndexed by the BINARY_OP oparg value.",
    ),
    (
        "get_intrinsic1_descs",
        "Return a list of names of the unary intrinsics.",
    ),
    (
        "get_intrinsic2_descs",
        "Return a list of names of the binary intrinsics.",
    ),
    (
        "get_executor",
        "Return the executor object at offset in code if exists, None otherwise.",
    ),
    (
        "get_special_method_names",
        "Return a list of special method names.",
    ),
];

pub(in crate::dynamic) fn doc(name: &str) -> Option<&'static str> {
    FUNCTIONS
        .iter()
        .find_map(|&(key, doc)| (key == name).then_some(doc))
}

pub(in crate::dynamic) fn install(ctx: &mut Context, module: Id, scope: Id) {
    for &(name, _) in FUNCTIONS {
        let callable = ctx.builtin(&format!("_opcode.{name}"));
        let function = ctx.alloc(Value::Method {
            callable,
            receiver: module,
            explicit: false,
        });
        ctx.bind(scope, name, function);
    }
}

/// Argument Clinic checks the total before it binds positional and keyword arguments.
fn check_max(name: &str, max: usize, args: &[Id], kwargs: &[(String, Id)]) -> Result<()> {
    let total = args.len() + kwargs.len();
    if total > max {
        let keyword = if args.is_empty() { "keyword " } else { "" };
        let plural = if max == 1 { "" } else { "s" };
        return Err(Error::new(
            "TypeError",
            format!("{name}() takes at most {max} {keyword}argument{plural} ({total} given)"),
        ));
    }
    Ok(())
}

fn required(
    name: &str,
    param: &str,
    index: usize,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    args.get(index)
        .copied()
        .or_else(|| kwargs.iter().find(|(k, _)| k == param).map(|(_, v)| *v))
        .ok_or_else(|| {
            Error::new(
                "TypeError",
                format!(
                    "{name}() missing required argument '{param}' (pos {})",
                    index + 1
                ),
            )
        })
}

fn c_int(ctx: &mut Context, value: Id) -> Result<i32> {
    ctx.index_integer(value)?
        .to_i64()
        .and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| Error::new("OverflowError", "Python int too large to convert to C int"))
}

fn stack_effect(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    check_max("stack_effect", 3, args, kwargs)?;
    if args.is_empty() {
        return Err(Error::new(
            "TypeError",
            "stack_effect() takes at least 1 positional argument (0 given)",
        ));
    }
    if args.len() > 2 {
        return Err(Error::new(
            "TypeError",
            "stack_effect() takes at most 2 positional arguments (3 given)",
        ));
    }
    let mut jump = NONE;
    for (key, value) in kwargs {
        if key != "jump" {
            let mut message = format!("stack_effect() got an unexpected keyword argument '{key}'");
            if let Some(hint) = crate::objects::exceptions::keyword_suggestion(key, &["jump"]) {
                message.push_str(&format!(". Did you mean '{hint}'?"));
            }
            return Err(Error::new("TypeError", message));
        }
        jump = *value;
    }
    let opcode = c_int(ctx, args[0])?;
    let oparg = match args.get(1) {
        Some(&value) if value != NONE => {
            // CPython converts the operand to C long before narrowing to C int.
            std::ffi::c_long::try_from(ctx.index_integer(value)?.big().as_ref()).map_err(|_| {
                Error::new("OverflowError", "Python int too large to convert to C long")
            })? as i32
        }
        _ => 0,
    };
    let branch = match jump {
        NONE => -1,
        FALSE => 0,
        TRUE => 1,
        _ => {
            return Err(Error::new(
                "ValueError",
                "stack_effect: jump must be False, True or None",
            ));
        }
    };
    metadata::stack_effect(opcode, oparg, branch)
        .map(|effect| ctx.int(i64::from(effect)))
        .ok_or_else(|| Error::new("ValueError", "invalid opcode or oparg"))
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    match name {
        "stack_effect" => stack_effect(ctx, args, kwargs),
        "is_valid" | "has_arg" | "has_const" | "has_name" | "has_jump" | "has_free"
        | "has_local" | "has_exc" => {
            check_max(name, 1, args, kwargs)?;
            let opcode = required(name, "opcode", 0, args, kwargs)?;
            Ok(super::builtins::boolean(metadata::predicate(
                name,
                c_int(ctx, opcode)?,
            )))
        }
        "get_executor" => {
            check_max(name, 2, args, kwargs)?;
            let code = required(name, "code", 0, args, kwargs)?;
            let offset = required(name, "offset", 1, args, kwargs)?;
            c_int(ctx, offset)?;
            if !matches!(ctx.heap.get(code), Value::Code(_)) {
                return Err(Error::new(
                    "TypeError",
                    format!("expected a code object, not '{:.100}'", ctx.type_name(code)),
                ));
            }
            // A native executable has no CPython Tier 2 executor.
            Err(Error::new(
                "RuntimeError",
                "Executors are not available in this build",
            ))
        }
        _ => {
            if !kwargs.is_empty() {
                return Err(Error::new(
                    "TypeError",
                    format!("_opcode.{name}() takes no keyword arguments"),
                ));
            }
            if !args.is_empty() {
                return Err(Error::new(
                    "TypeError",
                    format!("_opcode.{name}() takes no arguments ({} given)", args.len()),
                ));
            }
            let values = match name {
                "get_specialization_stats" => return Ok(NONE),
                "get_nb_ops" => metadata::NB_OPS
                    .iter()
                    .map(|&(name, symbol)| {
                        let name = ctx.text(name);
                        let symbol = ctx.text(symbol);
                        ctx.alloc(Value::Tuple(vec![name, symbol]))
                    })
                    .collect(),
                "get_intrinsic1_descs" => string_ids(ctx, metadata::INTRINSIC1),
                "get_intrinsic2_descs" => string_ids(ctx, metadata::INTRINSIC2),
                "get_special_method_names" => metadata::SPECIAL_METHODS
                    .iter()
                    .map(|s| ctx.heap.code_name(s))
                    .collect(),
                _ => {
                    return Err(Error::new(
                        "AttributeError",
                        format!("module '_opcode' has no attribute '{name}'"),
                    ));
                }
            };
            Ok(ctx.alloc(Value::List(values)))
        }
    }
}

fn string_ids(ctx: &mut Context, names: &[&str]) -> Vec<Id> {
    names.iter().map(|s| ctx.text(*s)).collect()
}
