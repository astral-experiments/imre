use super::{Context, Error, Id, NONE, Result, THREAD, Value};
use crate::dynamic::ffi;
use crate::dynamic::value::{Body, Code, TRUE};
use std::sync::Arc;

fn initialize(ctx: &mut Context, scope: Id) -> Result<Id> {
    let object = ctx.lookup(scope, "self")?;
    let dict = ctx.lookup_attr(object, "__dict__")?;
    if ctx.lookup(scope, "value")? != ctx.lookup(scope, "pos")?
        || ctx.lookup(scope, "option")? != ctx.lookup(scope, "kw")?
        || ctx.truth(dict)?
    {
        return Err(Error::new(
            "AssertionError",
            "unexpected arguments or dictionary",
        ));
    }
    let seen = ctx.lookup(scope, "seen")?;
    let append = ctx.lookup_attr(seen, "append")?;
    ctx.call(append, vec![dict], vec![])?;
    let attempt = ctx.items(seen)?.len() as i64;
    let value = ctx.int(attempt);
    ctx.set_attr(object, "attempt", value)?;
    if attempt == 2 {
        let message = ctx.text("retry");
        return Err(ctx.exception("ValueError", &[message]));
    }
    Ok(NONE)
}

unsafe extern "C" fn init(raw: *mut Context, scope: Id) -> Id {
    // SAFETY: execute exclusively owns the context and roots the bound arguments.
    let ctx = unsafe { &mut *raw };
    match initialize(ctx, scope) {
        Ok(result) => result,
        Err(mut error) => {
            ctx.record_exception_frame(&mut error, None);
            ctx.heap.execution.pending = Some(error);
            NONE
        }
    }
}

fn native_function(ctx: &mut Context, scope: Id, name: &str, signature: &str, entry: Code) -> Id {
    let name = ctx.text(name);
    let signature = ctx.text(signature);
    let filename = ctx.text("local_test.py");
    let code = ctx.prelinked_code(
        Body::Native(entry, NONE),
        scope,
        name,
        signature,
        (name, NONE),
        (filename, 1),
    );
    let defaults = ctx.dict(Vec::new());
    ctx.define_function(code, scope, name, defaults, name, NONE, NONE, false)
        .unwrap()
}

fn subclass(ctx: &mut Context, scope: Id, name: &str, parent: Id) -> Id {
    unsafe extern "C" fn empty_body(_: *mut Context, _: Id) -> Id {
        NONE
    }
    let body = native_function(ctx, scope, name, "{}", empty_body);
    let bases = ctx.alloc(Value::Tuple(vec![parent]));
    ctx.class(bases, body, vec![], false).unwrap()
}

#[test]
fn first_access_retries_inherited_initialization_in_a_fresh_dictionary() {
    let foreign_owner = THREAD.with(Arc::downgrade);
    std::thread::spawn(move || {
        let mut ctx = ffi::new_context(
            serde_json::json!({"paths": []}),
            std::iter::empty::<String>(),
        );
        let scope = ctx.scope("local_test", None);
        ctx.heap.keep(scope);
        let seen = ctx.alloc(Value::List(Vec::new()));
        let pos = ctx.alloc(Value::List(Vec::new()));
        let kw = ctx.dict(Vec::new());
        ctx.bind(scope, "seen", seen);
        ctx.bind(scope, "pos", pos);
        ctx.bind(scope, "kw", kw);

        let init = native_function(
            &mut ctx,
            scope,
            "__init__",
            r#"{"positional":["self","value"],"kwonly":["option"]}"#,
            init,
        );
        let local_type = ctx.builtin("_thread._local");
        let base = subclass(&mut ctx, scope, "Base", local_type);
        ctx.set_attr(base, "__init__", init).unwrap();
        let local = subclass(&mut ctx, scope, "Local", base);
        let object = ctx
            .call(local, vec![pos], vec![("option".into(), kw)])
            .unwrap();
        ctx.bind(scope, "local", object);
        let creator = ctx.lookup_attr(object, "__dict__").unwrap();
        ctx.bind(scope, "creator", creator);
        assert_eq!(ctx.items(seen).unwrap(), [creator]);
        let one = ctx.int(1);
        assert_eq!(ctx.dict_named_item(creator, "attempt"), Some(one));
        let storage = ctx.scalar_value(object);
        let Value::ThreadLocal(local) = ctx.heap.get_mut(storage) else {
            unreachable!()
        };
        // Keep the heap on this worker. Only the existing dictionary's live thread owner
        // changes, giving the worker an ordinary first access to the constructed local.
        local.threads[0].0 = foreign_owner;
        let error = ctx.lookup_attr(object, "__dict__").unwrap_err();
        assert!(ctx.is_builtin_error(&error, "ValueError"), "{error:?}");
        assert_eq!(error.message, "retry");

        ctx.set_attr(object, "after_retry", TRUE).unwrap();
        let current = ctx.lookup_attr(object, "__dict__").unwrap();
        let attempt = ctx.lookup_attr(object, "attempt").unwrap();
        assert_eq!(attempt, ctx.int(3));
        let dictionaries = ctx.items(seen).unwrap();
        assert_eq!(dictionaries.len(), 3);
        assert_eq!(dictionaries[0], creator);
        assert_eq!(dictionaries[2], current);
        for (index, dict) in dictionaries.into_iter().enumerate() {
            let expected = ctx.int(index as i64 + 1);
            assert_eq!(ctx.dict_named_item(dict, "attempt"), Some(expected));
            assert_eq!(
                ctx.items(dict).unwrap().len(),
                if index == 2 { 2 } else { 1 }
            );
        }
        assert_eq!(ctx.dict_named_item(current, "after_retry"), Some(TRUE));
    })
    .join()
    .unwrap();
}
