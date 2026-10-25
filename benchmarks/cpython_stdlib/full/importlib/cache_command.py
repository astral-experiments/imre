"""Probe interpreter optimization separately from the selected LLVM optimization."""
import sys
import warnings
from importlib.machinery import ModuleSpec
from importlib.util import cache_from_source, source_from_cache


def optional(path: str, optimization: str | None) -> str:
    return cache_from_source(path, optimization=optimization)


def check() -> None:
    sys.implementation.cache_tag = "test-tag"
    sys.pycache_prefix = None
    warnings.simplefilter("ignore", DeprecationWarning)
    path = "pkg/mod.py"
    print(sys.flags.optimize)
    print(cache_from_source(path))
    print(cache_from_source(path, optimization=None))
    print(optional(path, None))
    print(optional(path, "chosen"))
    print(cache_from_source(path, optimization=""))
    print(cache_from_source(path, optimization=0))
    print(cache_from_source(path, True))
    print(cache_from_source(path, False))
    spec = ModuleSpec("pkg.mod", None, origin=path)
    spec.has_location = True
    cached = spec.cached
    if cached is not None:
        print(cached)
        print(source_from_cache(cached))
    spec.origin = "other.py"
    print(spec.cached is cached)
    spec.cached = None
    changed = spec.cached
    if changed is not None:
        print(changed)


check()
