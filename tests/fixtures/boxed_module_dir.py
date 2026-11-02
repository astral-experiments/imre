from builtins import dir as directory

MODULE_NUMBER = 1
REMOVED_NUMBER = 2
del REMOVED_NUMBER
namespace = globals()
namespace["INJECTED_NUMBER"] = 3
assert [name for name in directory() if name.endswith("_NUMBER")] == ["INJECTED_NUMBER", "MODULE_NUMBER"]
assert directory() == sorted(namespace)
first = directory()
del namespace["INJECTED_NUMBER"]
assert "INJECTED_NUMBER" in first and "INJECTED_NUMBER" not in dir()
assert list(name for name in dir() if name.endswith("_NUMBER")) == ["MODULE_NUMBER"]

dir = lambda: ["shadowed"]
assert dir() == ["shadowed"] and directory() == sorted(namespace)
del dir


def current():
    pass
