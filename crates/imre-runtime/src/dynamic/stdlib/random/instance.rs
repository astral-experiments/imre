//! Random instances own independent MT19937 streams; the module singleton shares the native stream.
use std::cell::RefCell;
use std::rc::Rc;

use super::{Context, Error, Id, NONE, Result, Value, objects};
use crate::Int;

pub(in crate::dynamic) const CLASS: &str = "random.Random";

#[derive(Clone)]
pub(in crate::dynamic) enum RandomState {
    Local(Rc<RefCell<objects::Generator>>),
    Shared,
}

pub(in crate::dynamic) fn install(ctx: &mut Context, scope: Id) -> Result<()> {
    let class = ctx.scope_name(scope, "Random").expect("Random class");
    let Value::Class {
        heap_type,
        scope: fields,
        ..
    } = ctx.heap.get_mut(class)
    else {
        unreachable!("Random class")
    };
    *heap_type = Some(CLASS);
    let fields = *fields;
    for name in ["__init__", "seed", "random", "getrandbits"] {
        let method = ctx.alloc(Value::HeapTypeMethod { owner: class, name });
        ctx.bind(fields, name, method);
    }
    let singleton = ctx.instance(class, None);
    attach_state(ctx, singleton, RandomState::Shared);
    ctx.set_attr(singleton, "gauss_next", NONE)?;
    ctx.bind(scope, "_inst", singleton);
    for name in [
        "seed",
        "random",
        "getrandbits",
        "randbytes",
        "sample",
        "expovariate",
        "uniform",
    ] {
        let bound = ctx.lookup_attr(singleton, name)?;
        ctx.bind(scope, name, bound);
    }
    Ok(())
}

/// Keep MT state outside the attribute dictionary, including on subclasses.
fn attach_state(ctx: &mut Context, receiver: Id, state: RandomState) -> Id {
    let storage = ctx.alloc(Value::Random(state));
    let Value::Instance { scalar, .. } = ctx.heap.get_mut(receiver) else {
        unreachable!("checked Random receiver")
    };
    *scalar = Some(storage);
    storage
}

fn state(ctx: &mut Context, receiver: Id) -> &mut RandomState {
    let mut storage = ctx.scalar_value(receiver);
    if !matches!(ctx.heap.get(storage), Value::Random(_)) {
        // A subclass can override seed or skip __init__, leaving its native state zeroed.
        let local = Rc::new(RefCell::new(objects::Generator::unseeded()));
        storage = attach_state(ctx, receiver, RandomState::Local(local));
    }
    let Value::Random(state) = ctx.heap.get_mut(storage) else {
        unreachable!()
    };
    state
}

/// Draw from the receiver's current stream, including after a Python callback reseeds it.
pub(super) fn draw(ctx: &mut Context, receiver: Id) -> f64 {
    match state(ctx, receiver) {
        RandomState::Local(local) => local.borrow_mut().random(),
        RandomState::Shared => objects::imre_random_float(),
    }
}

/// Identify the captured native random method, including one inherited or borrowed.
pub(super) fn bound_stream(ctx: &Context, method: Id) -> Option<(Id, Id)> {
    let Value::Method {
        callable, receiver, ..
    } = *ctx.heap.get(method)
    else {
        return None;
    };
    let Value::HeapTypeMethod {
        owner,
        name: "random",
    } = *ctx.heap.get(callable)
    else {
        return None;
    };
    (ctx.heap.get(owner).heap_type() == Some(CLASS)).then_some((owner, receiver))
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
            match state(ctx, receiver) {
                RandomState::Local(local) => *local.borrow_mut() = generator,
                RandomState::Shared => objects::seed_shared(generator),
            }
            ctx.set_attr(receiver, "gauss_next", NONE)?;
            Ok(NONE)
        }
        "random" => {
            super::random_args(rest, kwargs)?;
            let value = draw(ctx, receiver);
            Ok(ctx.alloc(Value::Float(value)))
        }
        "getrandbits" => {
            let width = super::bit_count(ctx, rest, kwargs)?;
            let value = match state(ctx, receiver) {
                RandomState::Local(local) => local.borrow_mut().getrandbits(&width),
                RandomState::Shared => objects::random_bits(&width),
            }?;
            Ok(ctx.alloc(Value::Int(Int::new(value))))
        }
        _ => unreachable!("registered Random method"),
    }
}
