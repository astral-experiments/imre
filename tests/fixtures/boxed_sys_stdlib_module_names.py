import sys
from sys import stdlib_module_names as names


def current():
    assert sys.__dict__["stdlib_module_names"] is names
    assert type(names) is frozenset and "sys" in names
    try:
        sys.stdlib_module_names = frozenset()
        assert not sys.stdlib_module_names and names
        del sys.stdlib_module_names
        assert not hasattr(sys, "stdlib_module_names")
    finally:
        sys.stdlib_module_names = names

    del sys.modules["sys"]
    try:
        fresh = __import__("sys")
        assert not hasattr(fresh, "stdlib_module_names")
    finally:
        sys.modules["sys"] = sys
    print(len(names))
