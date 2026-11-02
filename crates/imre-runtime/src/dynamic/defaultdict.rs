//! Own native methods exported by collections.defaultdict.

pub(super) const CLASS: &str = "collections.defaultdict";
pub(super) const METHODS: &[&str] = &[
    "__repr__",
    "__init__",
    "__missing__",
    "copy",
    "__copy__",
    "__reduce__",
];

/// Return the CPython 3.14.5 documentation and text signature of an implemented descriptor.
pub(super) fn metadata(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
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
