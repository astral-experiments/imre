//! Populate an ordinary fresh matcher from authenticated source methods.

use super::{Context, DifflibBody, Id, NONE, Result, Value};

/// Fuse the canonical constructor's virtual method chain after all observable hooks are excluded.
pub(super) fn attempt(ctx: &mut Context, scope: Id) -> Result<Option<Id>> {
    let Some(receiver) = super::receiver(ctx, scope) else {
        return Ok(None);
    };
    let root = ctx.root_scope(scope);
    if !super::canonical_builtins(ctx, root, &["enumerate", "set", "len"]) {
        return Ok(None);
    }
    let Value::Instance {
        class,
        fields,
        namespace,
        ..
    } = ctx.heap.get(receiver)
    else {
        unreachable!()
    };
    let class = *class;
    if !fields.is_empty()
        || namespace.is_some()
        || !ctx.class_instances_have_dict(class)
        || ctx.allocation_base(class) != ctx.builtins.get("object").copied()
        || ctx.class_attr(class, "__getattribute__").is_some()
        || ctx.class_attr(class, "__getattr__").is_some()
    {
        return Ok(None);
    }

    // Source assignment and virtual lookup can invoke class descriptors or
    // user code. Guard every participating method and every written field
    // before publishing the first value.
    for (name, body) in [
        ("__init__", DifflibBody::Init),
        ("set_seqs", DifflibBody::SetSeqs),
        ("set_seq1", DifflibBody::SetSeq1),
        ("set_seq2", DifflibBody::SetSeq2),
        ("_SequenceMatcher__chain_b", DifflibBody::ChainB),
    ] {
        if !super::trusted_method(ctx, class, root, name, body) {
            return Ok(None);
        }
    }
    const FIELDS: [&str; 10] = [
        "isjunk",
        "a",
        "b",
        "autojunk",
        "matching_blocks",
        "opcodes",
        "fullbcount",
        "b2j",
        "bjunk",
        "bpopular",
    ];
    if FIELDS
        .iter()
        .any(|name| ctx.class_attr(class, name).is_some())
    {
        return Ok(None);
    }

    let Some(isjunk) = ctx.scope_name(scope, "isjunk") else {
        return Ok(None);
    };
    let Some(a) = ctx.scope_name(scope, "a") else {
        return Ok(None);
    };
    let Some(b) = ctx.scope_name(scope, "b") else {
        return Ok(None);
    };
    let Some(autojunk) = ctx.scope_name(scope, "autojunk") else {
        return Ok(None);
    };
    if isjunk != NONE || !matches!(ctx.heap.get(a), Value::Str(_)) {
        return Ok(None);
    }
    let Value::Str(text) = ctx.heap.get(b) else {
        return Ok(None);
    };
    if !text.is_ascii() {
        return Ok(None);
    }
    let Value::Bool(use_autojunk) = ctx.heap.get(autojunk) else {
        return Ok(None);
    };
    let use_autojunk = *use_autojunk;
    if !super::unobserved_checkpoints(ctx) {
        return Ok(None);
    }

    // With an unexposed empty dictionary, ordinary object storage, no hooks,
    // and only exact immutable inputs, intermediate overwrites cannot be
    // observed. Publish the final values in the source's insertion order.
    let Value::Instance { fields, .. } = ctx.heap.get_mut(receiver) else {
        unreachable!()
    };
    fields
        .try_reserve(FIELDS.len())
        .map_err(|_| crate::Error::memory_error())?;
    for (name, value) in [
        ("isjunk", isjunk),
        ("a", a),
        ("b", b),
        ("autojunk", autojunk),
        ("matching_blocks", NONE),
        ("opcodes", NONE),
        ("fullbcount", NONE),
    ] {
        ctx.heap.store_instance_field(receiver, name, value);
    }
    super::build_chain_ascii(ctx, receiver, b, use_autojunk)?;
    Ok(Some(NONE))
}
