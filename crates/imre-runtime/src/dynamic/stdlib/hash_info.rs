//! The runtime's hash configuration, presented as Python's nine-field struct sequence.

use super::{Context, Id, structseq::HASH_INFO};
use crate::python_hash;

pub(super) fn current(ctx: &mut Context) -> Id {
    let values = vec![
        ctx.int(i64::from(i64::BITS)),
        ctx.int(python_hash::MODULUS),
        ctx.int(python_hash::INF),
        // Retained for compatibility; NaNs hash by identity.
        ctx.int(0),
        ctx.int(python_hash::IMAG),
        ctx.text(python_hash::ALGORITHM),
        ctx.int(i64::from(python_hash::HASH_BITS)),
        ctx.int(i64::from(python_hash::SEED_BITS)),
        ctx.int(i64::from(python_hash::CUTOFF)),
    ];
    HASH_INFO.record(ctx, values)
}
