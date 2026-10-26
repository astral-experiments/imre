"""Compare every standard library name in fresh native and CPython processes."""
import sys
from collections.abc import Set
from sys import stdlib_module_names as names


def view_size(value: Set[str]) -> int:
    return len(value)


def truth(value: frozenset[str] | str) -> bool:
    return bool(value)


def main() -> None:
    print(len(names))
    print(isinstance(names, frozenset))
    print(isinstance(names, set))
    print(names is sys.stdlib_module_names)
    print(frozenset(names) is names)
    print(names.copy() is names)
    print("sys" in names)
    print("winreg" in names)
    print("test" in names)
    print("email.message" in names)
    ordered: list[str] = []
    for name in names:
        if not isinstance(name, str):
            raise AssertionError("non-string module name")
        ordered.append(name)
    ordered.sort()
    for name in ordered:
        print(name)
    exact = frozenset(["sys", "sys"])
    print(repr(exact))
    print(exact == frozenset(["sys"]))
    print(exact != frozenset(["os"]))
    print(isinstance(exact, frozenset))
    print(isinstance(exact, set))
    print(view_size(names))
    empty: list[str] = []
    print(repr(frozenset(empty)))
    print(truth(exact))
    print(truth(frozenset(empty)))
    print(truth(""))


main()
