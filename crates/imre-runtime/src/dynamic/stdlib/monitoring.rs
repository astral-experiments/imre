//! Public constants on the per-interpreter sys.monitoring module (CPython 3.14).

use super::{Context, Id, Result};
use crate::dynamic::namespace;

// The event index is the bit position in CPython's public event mask.
const EVENTS: [&str; 19] = [
    "PY_START",
    "PY_RESUME",
    "PY_RETURN",
    "PY_YIELD",
    "CALL",
    "LINE",
    "INSTRUCTION",
    "JUMP",
    "BRANCH_LEFT",
    "BRANCH_RIGHT",
    "STOP_ITERATION",
    "RAISE",
    "EXCEPTION_HANDLED",
    "PY_UNWIND",
    "PY_THROW",
    "RERAISE",
    "C_RETURN",
    "C_RAISE",
    "BRANCH",
];

pub(in crate::dynamic) fn module(ctx: &mut Context) -> Result<Id> {
    let class = ctx.builtin("module");
    let module = ctx.allocate_module(class);
    let name = ctx.text("sys.monitoring");
    ctx.init_module(module, &[name], &[])?;

    let object = ctx.builtin("object");
    for name in ["DISABLE", "MISSING"] {
        let sentinel = ctx.instance(object, None);
        ctx.set_attr(module, name, sentinel)?;
    }

    let class = namespace::class(ctx)?;
    let events = ctx.instance(class, None);
    for (index, name) in EVENTS.iter().enumerate() {
        let mask = ctx.int(1_i64 << index);
        ctx.set_attr(events, name, mask)?;
    }
    let zero = ctx.int(0);
    ctx.set_attr(events, "NO_EVENTS", zero)?;
    ctx.set_attr(module, "events", events)?;

    for (name, number) in [
        ("DEBUGGER_ID", 0),
        ("COVERAGE_ID", 1),
        ("PROFILER_ID", 2),
        ("OPTIMIZER_ID", 5),
    ] {
        let id = ctx.int(number);
        ctx.set_attr(module, name, id)?;
    }
    Ok(module)
}
