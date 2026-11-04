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
    let version = ctx.int(3);
    ctx.bind(fields, "VERSION", version);
    const METHODS: &[&str] = &[
        "seed",
        "random",
        "getrandbits",
        "gauss",
        "getstate",
        "setstate",
    ];
    for name in std::iter::once("__init__").chain(METHODS.iter().copied()) {
        let method = ctx.alloc(Value::HeapTypeMethod { owner: class, name });
        ctx.bind(fields, name, method);
    }
    ctx.bind(scope, "Random", class);
    super::gaussian::install_aliases(ctx, scope)?;
    let singleton = ctx.instance(class, None);
    attach_state(ctx, singleton, RandomState::Shared);
    ctx.set_attr(singleton, "gauss_next", NONE)?;
    ctx.bind(scope, "_inst", singleton);
    for &name in METHODS {
        let bound = ctx.lookup_attr(singleton, name)?;
        ctx.bind(scope, name, bound);
    }
    #[cfg(target_os = "linux")]
    if super::super::os::fork_available() && !ctx.threads.private_context {
        // random.py registers the original bound seed in import order.
        let seed = ctx.lookup_attr(singleton, "seed")?;
        crate::dynamic::at_fork::register(ctx, &[], &[("after_in_child".into(), seed)])?;
        ctx.random_fork_hook = Some(seed);
    }
    Ok(())
}

/// An immediate exec discards both the child's MT state and its ordinary scalar cache.
/// Leave observable attribute writes and finalizers to the normal after-fork path.
#[cfg(target_os = "linux")]
pub(in crate::dynamic::stdlib) fn can_skip_reseed_before_exec(ctx: &Context, hook: Id) -> bool {
    let Value::Method {
        callable, receiver, ..
    } = *ctx.heap.get(hook)
    else {
        return false;
    };
    let Value::HeapTypeMethod {
        owner,
        name: "seed",
    } = *ctx.heap.get(callable)
    else {
        return false;
    };
    let Value::Instance {
        class, namespace, ..
    } = *ctx.heap.get(receiver)
    else {
        return false;
    };
    let object = ctx.builtins["object"];
    class == owner
        && ctx.class_attr(class, "__setattr__") == ctx.class_attr(object, "__setattr__")
        && ctx.class_attr(class, "gauss_next").is_none()
        && namespace.is_none_or(|dict| {
            ctx.heap.dict_entries(dict).is_some_and(|items| {
                items
                    .iter()
                    .all(|&(key, _)| matches!(ctx.heap.get(key), Value::Str(_)))
            })
        })
        && ctx
            .raw_instance_field(receiver, "gauss_next")
            .is_some_and(|cache| matches!(ctx.heap.get(cache), Value::None | Value::Float(_)))
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

pub(super) fn state(ctx: &mut Context, receiver: Id) -> &mut RandomState {
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

pub(in crate::dynamic) fn call(
    ctx: &mut Context,
    owner: Id,
    method: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if method == "gauss" {
        let [receiver, mu, sigma] = super::super::python_arguments(
            "Random.gauss",
            ["self", "mu", "sigma"],
            3,
            2,
            args,
            kwargs,
        )?;
        let Value::Class { scope, .. } = *ctx.heap.get(owner) else {
            unreachable!("Random method owner")
        };
        let globals = ctx.root_scope(scope);
        return super::gaussian::gauss(ctx, globals, receiver.expect("required self"), mu, sigma);
    }
    if method == "getstate" {
        let [receiver] =
            super::super::required_arguments("Random.getstate", ["self"], args, kwargs)?;
        ctx.check_heap_method_receiver(owner, method, receiver)?;
        return super::state::getstate(ctx, receiver);
    }
    if method == "setstate" {
        let [receiver, state] =
            super::super::required_arguments("Random.setstate", ["self", "state"], args, kwargs)?;
        ctx.check_heap_method_receiver(owner, method, receiver)?;
        return super::state::setstate(ctx, receiver, state);
    }
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
            let value = match state(ctx, receiver) {
                RandomState::Local(local) => local.borrow_mut().random(),
                RandomState::Shared => objects::imre_random_float(),
            };
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
