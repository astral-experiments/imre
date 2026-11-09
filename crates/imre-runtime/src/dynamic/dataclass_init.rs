//! One native body for the functions created by the dataclasses decorator.
//! The ordinary Python binder has filled the frame before this entry runs.
use super::value::Body;
use super::{Context, Error, Id, NONE, Result, TRUE, Value, ffi};

fn invalid() -> Error {
    Error::new("TypeError", "invalid dataclass initializer operands")
}

fn tuple(ctx: &Context, id: Id) -> Result<&[Id]> {
    match ctx.heap.get(id) {
        Value::Tuple(items) => Ok(items),
        _ => Err(invalid()),
    }
}

fn name(ctx: &Context, id: Id) -> Result<crate::Str> {
    match ctx.heap.get(id) {
        Value::Str(name) => Ok(name.clone()),
        _ => Err(invalid()),
    }
}

fn names(ctx: &Context, values: &[Id]) -> Result<Vec<String>> {
    values
        .iter()
        .map(|&id| name(ctx, id).map(|s| s.to_string()))
        .collect()
}

fn bound(ctx: &mut Context, scope: Id, key: Id) -> Result<Id> {
    let value = ctx.lookup(scope, &name(ctx, key)?)?;
    ctx.heap.keep(value);
    Ok(value)
}

// A user can replace the closure cell, including with a Python subscriptable
// object. Load it at the point where the original expression needs its value.
fn captured(ctx: &mut Context, scope: Id, index: Id) -> Result<Id> {
    let cell = ctx
        .code_object(ctx.heap.current_code())
        .freevars
        .first()
        .cloned()
        .ok_or_else(invalid)?;
    let constants = ctx.lookup(scope, &cell)?;
    ctx.heap.keep(constants);
    let value = ctx.get_item(constants, index)?;
    ctx.heap.keep(value);
    Ok(value)
}

fn call(ctx: &mut Context, scope: Id, function: Id, args: Vec<Id>) -> Result<Id> {
    let value = ctx.call_in_scope(scope, function, args, Vec::new())?;
    ctx.check_call_return(value)
}

fn store(ctx: &mut Context, scope: Id, receiver: Id, field: Id) -> Result<()> {
    let &[attr, argument, index, factory, frozen] = tuple(ctx, field)? else {
        return Err(invalid());
    };
    // A frozen assignment evaluates the callable and receiver before the RHS.
    // Ordinary attribute assignment evaluates the RHS before its receiver.
    let setter = if frozen == TRUE {
        let one = ctx.int(1);
        Some((captured(ctx, scope, one)?, bound(ctx, scope, receiver)?))
    } else {
        None
    };
    let value = if factory == TRUE {
        let use_factory = if argument == NONE {
            true
        } else {
            let value = bound(ctx, scope, argument)?;
            let zero = ctx.int(0);
            value == captured(ctx, scope, zero)?
        };
        if use_factory {
            let factory = captured(ctx, scope, index)?;
            call(ctx, scope, factory, Vec::new())?
        } else {
            bound(ctx, scope, argument)?
        }
    } else if index == NONE {
        bound(ctx, scope, argument)?
    } else {
        captured(ctx, scope, index)?
    };
    ctx.heap.keep(value);
    if let Some((setter, owner)) = setter {
        call(ctx, scope, setter, vec![owner, attr, value])?;
    } else {
        let owner = bound(ctx, scope, receiver)?;
        ctx.set_attr(owner, &name(ctx, attr)?, value)?;
    }
    Ok(())
}

fn statement(ctx: &mut Context, offset: usize) -> Result<()> {
    let line = u32::try_from(offset).unwrap_or(u32::MAX).saturating_add(3);
    // SAFETY: the filename is static. All values live across statements are in the frame.
    unsafe { ctx.native_statement(&[], c"<string>".as_ptr(), line, NONE) }
}

fn run(ctx: &mut Context, scope: Id) -> Result<Id> {
    let Body::Native(_, data) = ctx.code_object(ctx.heap.current_code()).body else {
        return Err(invalid());
    };
    let &[receiver, stores, post_init] = tuple(ctx, data)? else {
        return Err(invalid());
    };
    let count = tuple(ctx, stores)?.len();
    for i in 0..count {
        statement(ctx, i)?;
        let field = tuple(ctx, stores)?[i];
        store(ctx, scope, receiver, field)?;
    }
    if post_init != NONE {
        statement(ctx, count)?;
        let owner = bound(ctx, scope, receiver)?;
        let method = ctx.lookup_attr(owner, "__post_init__")?;
        ctx.heap.keep(method);
        let names = tuple(ctx, post_init)?.to_vec();
        let args = names
            .into_iter()
            .map(|key| bound(ctx, scope, key))
            .collect::<Result<_>>()?;
        call(ctx, scope, method, args)?;
    } else if count == 0 {
        statement(ctx, 0)?;
    }
    Ok(NONE)
}

pub(super) unsafe extern "C" fn initialize(raw: *mut Context, scope: Id) -> Id {
    // SAFETY: execute entered this ordinary function frame and owns the context.
    let ctx = unsafe { &mut *raw };
    let result = run(ctx, scope);
    ffi::finish(ctx, result)
}

/// Supply the callable ABI with this decoration's actual argument and closure bindings.
pub(super) fn bind_initializer(ctx: &mut Context, args: &[Id]) -> Result<Id> {
    let &[
        globals,
        positional,
        keyword,
        defaults,
        operands,
        closure_name,
        constants,
    ] = args
    else {
        return Err(invalid());
    };
    let (Value::List(positional), Value::List(keyword)) =
        (ctx.heap.get(positional), ctx.heap.get(keyword))
    else {
        return Err(invalid());
    };
    if positional.is_empty()
        || ctx.dict_storage(globals).is_none()
        || !matches!(ctx.heap.get(defaults), Value::Dict(_))
        || !matches!(ctx.heap.get(operands), Value::Tuple(_))
        || !matches!(ctx.heap.get(constants), Value::Tuple(_))
    {
        return Err(invalid());
    }
    let closure_name = match ctx.heap.get(closure_name) {
        Value::None => None,
        Value::Str(name) => Some(name.to_string()),
        _ => return Err(invalid()),
    };
    let varnames = positional.iter().chain(keyword).copied().collect();
    let positional = names(ctx, positional)?;
    let keyword = names(ctx, keyword)?;
    // Field flags can run Python between the decorator's first validation and
    // this final signature. Only a contiguous positional suffix has defaults.
    let mut optional = false;
    for param in &positional {
        if ctx.dict_named_item(defaults, param).is_some() {
            optional = true;
        } else if optional {
            return Err(Error::new(
                "SyntaxError",
                format!("parameter '{param}' without a default follows a parameter with a default"),
            ));
        }
    }
    let mut seen = std::collections::HashSet::new();
    for param in positional.iter().chain(&keyword) {
        if param == "__debug__" {
            return Err(Error::new("SyntaxError", "cannot assign to __debug__"));
        }
        if !seen.insert(param) {
            return Err(Error::new(
                "SyntaxError",
                format!("duplicate argument '{param}' in function definition"),
            ));
        }
    }
    let signature = serde_json::json!({
        "positional": positional,
        "kwonly": keyword,
        "scope_reads": closure_name.as_slice(),
        "scope_complete": true,
        "scope_locals_complete": true,
        "scope_cellvars": [],
        "code_flags": imre_ast::code_flags::function_flags(None, true, false),
    });
    let builtins = ctx.namespace_builtins(globals, None)?;
    let globals = ctx.function_environment("", globals, builtins);
    let scope = if let Some(name) = closure_name {
        let cell = ctx.alloc(Value::Cell(Some(constants)));
        ctx.closure_environment(globals, vec![name], &[cell])
    } else {
        globals
    };
    let name = ctx.text("__init__");
    let qualname = ctx.text("build.<locals>.__init__");
    let filename = ctx.text("<string>");
    let signature = ctx.text(signature.to_string());
    let code = ctx.prelinked_code(
        Body::Native(initialize, operands),
        scope,
        name,
        signature,
        (qualname, NONE),
        (filename, 2),
    );
    let varnames = ctx.alloc(Value::Tuple(varnames));
    let Value::Code(metadata) = ctx.heap.get_mut(code) else {
        unreachable!("prelinked code")
    };
    metadata.varnames = varnames;
    metadata.cellvars = super::value::EMPTY_TUPLE;
    metadata.module_definition = false;
    ctx.define_function(code, scope, name, defaults, qualname, NONE, NONE, false)
}
