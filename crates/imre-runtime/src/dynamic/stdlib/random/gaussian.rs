//! Random.gauss from CPython 3.14.5 Lib/random.py (see licenses/CPython.txt).
use super::{Context, Id, NONE, Result, Value};

pub(super) fn install_aliases(ctx: &mut Context, scope: Id) -> Result<()> {
    // Capture pre-import monkeypatches if math is already loaded. Otherwise the native
    // math provider supplies the same callables without importing a Python module.
    let loaded = ctx.cached_import("math")?;
    for (alias, name) in [
        ("_log", "log"),
        ("_sqrt", "sqrt"),
        ("_sin", "sin"),
        ("_cos", "cos"),
        ("TWOPI", "tau"),
    ] {
        let value = match loaded {
            Some(module) => {
                let key = ctx.text(name);
                ctx.import_from(module, key)?
            }
            None if name == "tau" => ctx.alloc(Value::Float(std::f64::consts::TAU)),
            None => super::super::attribute(ctx, "math", name)?,
        };
        ctx.bind(scope, alias, value);
    }
    Ok(())
}

pub(super) fn gauss(
    ctx: &mut Context,
    scope: Id,
    receiver: Id,
    mu: Option<Id>,
    sigma: Option<Id>,
) -> Result<Id> {
    let draw = ctx.lookup_attr(receiver, "random")?;
    ctx.heap.keep(draw);
    let mut z = ctx.lookup_attr(receiver, "gauss_next")?;
    ctx.heap.keep(z);
    ctx.set_attr(receiver, "gauss_next", NONE)?;
    if z == NONE {
        let first = ctx.call(draw, vec![], vec![])?;
        let tau = ctx.lookup(scope, "TWOPI")?;
        let angle = ctx.binary("*", first, tau)?;
        // Python resolves both outer callables before drawing the second uniform.
        let sqrt = ctx.lookup(scope, "_sqrt")?;
        ctx.heap.keep(sqrt);
        let log = ctx.lookup(scope, "_log")?;
        ctx.heap.keep(log);
        let second = ctx.call(draw, vec![], vec![])?;
        let one = ctx.alloc(Value::Float(1.0));
        let complement = ctx.binary("-", one, second)?;
        let log_value = ctx.call(log, vec![complement], vec![])?;
        let minus_two = ctx.alloc(Value::Float(-2.0));
        let square = ctx.binary("*", minus_two, log_value)?;
        let radius = ctx.call(sqrt, vec![square], vec![])?;
        let cos = ctx.lookup(scope, "_cos")?;
        let cosine = ctx.call(cos, vec![angle], vec![])?;
        z = ctx.binary("*", cosine, radius)?;
        let sin = ctx.lookup(scope, "_sin")?;
        let sine = ctx.call(sin, vec![angle], vec![])?;
        let next = ctx.binary("*", sine, radius)?;
        ctx.set_attr(receiver, "gauss_next", next)?;
    }
    let sigma = sigma.unwrap_or_else(|| ctx.alloc(Value::Float(1.0)));
    let scaled = ctx.binary("*", z, sigma)?;
    let mu = mu.unwrap_or_else(|| ctx.alloc(Value::Float(0.0)));
    ctx.binary("+", mu, scaled)
}
