"""Bridge only inputs; native code performs path, truth, string and warning operations."""
import sys
import warnings
from importlib.util import cache_from_source, source_from_cache
from pathlib import PurePath

# The harness inserts the unchanged nested class from PEP3147Tests.test_cache_from_source_override.
# UPSTREAM_BEARISH


def optional() -> str | None:
    if input() == "none":
        return None
    return input()


def calculate() -> str:
    sys.implementation.cache_tag = optional()
    sys.pycache_prefix = optional()
    policy = input()
    if policy != "native-default":
        warnings.simplefilter(policy, DeprecationWarning)
    op = input()
    path_kind = input()
    path = input()
    if op == "reverse":
        if path_kind == "pure":
            return source_from_cache(PurePath(path))
        return source_from_cache(path)
    debug = input()
    opt = input()
    level = input()
    if debug == "bool":
        flag = input() == "true"
        if opt == "str":
            return cache_from_source(path, flag, optimization=level)
        return cache_from_source(path, flag)
    if debug == "list":
        values: list[int] = []
        for i in range(int(input())):
            values.append(int(input()))
        return cache_from_source(path, values)
    if debug == "bearish":
        return cache_from_source(path, Bearish())
    if path_kind == "pure":
        return cache_from_source(PurePath(path), optimization=level)
    if opt == "int":
        return cache_from_source(path, optimization=int(level))
    if opt == "none":
        return cache_from_source(path, optimization=None)
    return cache_from_source(path, optimization=level)


try:
    value = calculate()
    print("ok")
    print(repr(value))
except DeprecationWarning as exc:
    print("DeprecationWarning")
    print(repr(str(exc)))
except NotImplementedError as exc:
    print("NotImplementedError")
    print(repr(str(exc)))
except RuntimeError as exc:
    print("RuntimeError")
    print(repr(str(exc)))
except ValueError as exc:
    print("ValueError")
    print(repr(str(exc)))
except TypeError as exc:
    print("TypeError")
    print(repr(str(exc)))
