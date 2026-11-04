//! The public Random class owns one MT19937 state per instance.
use std::cell::RefCell;
use std::rc::Rc;

use super::{Context, Error, Id, NONE, Result, Value, objects};
use crate::Int;

pub(in crate::dynamic) const CLASS: &str = "random.Random";

pub(in crate::dynamic) fn install(ctx: &mut Context, scope: Id) -> Result<()> {
    let fields = ctx.scope("Random", Some(scope));
    let module = ctx.text("random");
    let qualname = ctx.text("Random");
    ctx.bind(fields, "__module__", module);
    ctx.bind(fields, "__qualname__", qualname);
    let object = ctx.builtin("object");
    let class = ctx.allocate_class("Random".into(), fields, vec![object])?;
    let Value::Class { heap_type, .. } = ctx.heap.get_mut(class) else {
        unreachable!()
    };
    *heap_type = Some(CLASS);
    for name in ["__init__", "seed", "random", "getrandbits"] {
        let method = ctx.alloc(Value::HeapTypeMethod { owner: class, name });
        ctx.bind(fields, name, method);
    }
    ctx.bind(scope, "Random", class);
    Ok(())
}

/// Keep the generator out of the Python attribute dictionary, including on subclasses.
fn state(ctx: &mut Context, receiver: Id) -> Rc<RefCell<objects::Generator>> {
    if let Value::Random(generator) = ctx.heap.get(ctx.scalar_value(receiver)) {
        return Rc::clone(generator);
    }
    // A subclass can override seed or skip __init__, leaving the native state zeroed.
    let generator = Rc::new(RefCell::new(objects::Generator::unseeded()));
    let storage = ctx.alloc(Value::Random(Rc::clone(&generator)));
    let Value::Instance { scalar, .. } = ctx.heap.get_mut(receiver) else {
        unreachable!("checked Random receiver")
    };
    *scalar = Some(storage);
    generator
}

pub(in crate::dynamic) fn call(
    ctx: &mut Context,
    owner: Id,
    method: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&receiver, rest)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("unbound method Random.{method}() needs an argument"),
        ));
    };
    ctx.check_heap_method_receiver(owner, method, receiver)?;
    match method {
        "__init__" => {
            let [_, seed] = super::super::python_arguments(
                "Random.__init__",
                ["self", "x"],
                2,
                1,
                args,
                kwargs,
            )?;
            let reseed = ctx.lookup_attr(receiver, "seed")?;
            ctx.call(reseed, vec![seed.unwrap_or(NONE)], vec![])?;
            ctx.set_attr(receiver, "gauss_next", NONE)?;
            Ok(NONE)
        }
        "seed" => {
            let generator = super::seeded(ctx, args, kwargs)?;
            *state(ctx, receiver).borrow_mut() = generator;
            ctx.set_attr(receiver, "gauss_next", NONE)?;
            Ok(NONE)
        }
        "random" => {
            super::random_args(rest, kwargs)?;
            let value = state(ctx, receiver).borrow_mut().random();
            Ok(ctx.alloc(Value::Float(value)))
        }
        "getrandbits" => {
            let width = super::bit_count(ctx, rest, kwargs)?;
            let value = state(ctx, receiver).borrow_mut().getrandbits(&width)?;
            Ok(ctx.alloc(Value::Int(Int::new(value))))
        }
        _ => unreachable!("registered Random method"),
    }
}
