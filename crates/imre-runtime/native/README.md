The errno registration order, Winsock fallbacks, and module documentation follow
CPython v3.14.5 (commit 5607950ef232dad16d75c0cf53101d9649d89115,
Modules/errnomodule.c). The original license is in PSF-LICENSE. The entries
contain symbolic names only. build.rs compiles a table against the target's
actual errno headers so an absent platform constant is absent in the module.
