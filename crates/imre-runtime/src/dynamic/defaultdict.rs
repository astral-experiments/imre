//! Own native methods exported by collections.defaultdict.

use super::{
    Context,
    calls::MappingOperation,
    value::{Error, Id, NOT_IMPLEMENTED, Result},
};

pub(super) const CLASS: &str = "collections.defaultdict";
pub(super) const METHODS: &[&str] = &[
    "__repr__",
    "__init__",
    "__missing__",
    "__or__",
    "__ror__",
    "copy",
    "__copy__",
    "__reduce__",
];

/// Return the CPython 3.14.5 documentation and text signature of an implemented descriptor.
pub(super) fn metadata(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "__or__" => ("Return self|value.", "($self, value, /)"),
        "__ror__" => ("Return value|self.", "($self, value, /)"),
        "__repr__" => ("Return repr(self).", "($self, /)"),
        "__init__" => (
            "Initialize self.  See help(type(self)) for accurate signature.",
            "($self, /, *args, **kwargs)",
        ),
        "__missing__" => (
            "__missing__(key) # Called by __getitem__ for missing key; pseudo-code:\n  if self.default_factory is None: raise KeyError((key,))\n  self[key] = value = self.default_factory()\n  return value\n",
            "($self, object, /)",
        ),
        "copy" | "__copy__" => ("D.copy() -> a shallow copy of D.", "($self, /)"),
        "__reduce__" => ("Return state information for pickling.", "($self, /)"),
        _ => return None,
    })
}

/// Construct the union using the concrete type and factory of the first defaultdict operand.
pub(super) fn union(ctx: &mut Context, left: Id, right: Id) -> Result<Id> {
    if ctx.dict_storage(left).is_none() || ctx.dict_storage(right).is_none() {
        return Ok(NOT_IMPLEMENTED);
    }
    let owner = if ctx.defaultdict_factory(left).is_some() {
        left
    } else {
        right
    };
    let factory = ctx
        .defaultdict_factory(owner)
        .expect("validated defaultdict receiver");
    let class = ctx.exact_type(owner)?;
    let result = ctx.call(class, vec![factory, left], Vec::new())?;
    let storage = ctx
        .dict_storage(result)
        .ok_or_else(|| Error::new("SystemError", "bad argument to internal function"))?;
    // PyDict_Update merges the right mapping directly, bypassing the result's Python methods.
    ctx.for_each_mapping_item(
        right,
        storage,
        MappingOperation::Copy,
        |ctx, key, value, hash| {
            ctx.set_dictionary_key(storage, key, value, hash, true)
                .map(|_| ())
        },
    )?;
    Ok(result)
}
