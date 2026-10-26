"""Observe the target errno table, its aliases, and the shared mutable dictionary."""
import errno
from errno import EDOM as DOMAIN, ERANGE, errorcode as reverse, __name__ as module_name
from types import ModuleType, SimpleNamespace


def module_has_code(module: ModuleType, name: str) -> bool:
    return SimpleNamespace(present=hasattr(module, name)).present


def evaluate(value: int) -> int:
    print("evaluated")
    return value


def main() -> None:
    mod = errno
    assert module_has_code(mod, "EDOM")
    assert not module_has_code(mod, "NOT_AN_ERRNO_CODE")
    names = 0
    for name in mod.__dict__.keys():
        print(name)
        if name.isupper():
            value = getattr(mod, name)
            print(str(value))
            print(value in reverse)
            names += 1
    print(names)
    print(len(reverse))
    for name in reverse.values():
        print(name)
        print(hasattr(mod, name))
    print(DOMAIN == errno.EDOM)
    print(ERANGE == errno.ERANGE)
    print(ERANGE + 1)
    print(mod is errno)
    print(vars(mod) is errno.__dict__)
    key_dict = "errorcode"
    print(getattr(mod, key_dict) == reverse)
    print(reverse == getattr(mod, key_dict))
    try:
        for existing in reverse.values():
            print(existing)
            reverse[-93] = "new errno"
    except RuntimeError as changed:
        print(str(changed))
    del reverse[-93]
    print(module_name)
    print(errno.__name__)
    print(errno.__package__)
    print(getattr(errno, "EDOM"))
    print(hasattr(errno, "missing"))
    print(hasattr(errno, "__dict__"))
    name = "E_DOES_NOT_EXIST"
    try:
        print(repr(getattr(errno, name)))
    except AttributeError as exc:
        print(str(exc))
    try:
        print(repr(vars(errno)[name]))
    except KeyError as exc:
        print(str(exc))
    original = errno.errorcode[DOMAIN]
    reverse[DOMAIN] = "temporary"
    print(errno.errorcode[DOMAIN])
    errno.errorcode[DOMAIN] = original
    reverse[-99] = "test only"
    print(errno.errorcode[-99])
    del errno.errorcode[-99]
    print(-99 in reverse)
    print(repr(getattr(errno, "__loader__")))
    print(repr(getattr(errno, "__spec__")))
    dynamic = "__dict__"
    print(repr(getattr(errno, dynamic)))
    dynamic = "__class__"
    print(repr(getattr(errno, dynamic)))
    dynamic = "__dir__"
    print(repr(getattr(errno, dynamic)))
    for dynamic in ["__annotations__", "__annotate__"]:
        print(dynamic in vars(errno))
        print(hasattr(errno, dynamic))
        print(dynamic in vars(errno))
        print(repr(getattr(errno, dynamic)))
        print(repr(vars(errno)[dynamic]))
    print(repr(getattr(errno, "EDOM", evaluate(170141183460469231731687303715884105728))))
    print(repr(getattr(errno, "unknown_name", evaluate(170141183460469231731687303715884105728))))
    print(repr(getattr(errno, "unknown_name", b"fallback")))
    print(repr(getattr(errno, "unknown_name", None)))
    print(repr(getattr(errno, "unknown_name", True)))
    print(repr(getattr(errno, "unknown_name", "fallback")))
    for key in ["EDOM", "__package__", "__annotations__", "__annotate__", "__loader__"]:
        item = getattr(errno, key)
        print(bool(item))
        print(item is None)
        print(isinstance(item, int))
        print(isinstance(item, str))
        print(isinstance(item, dict))
    print(getattr(errno, "unknown_name", False) == 0)
    print(getattr(errno, "unknown_name", b"fallback") == b"fallback")
    for name in ["errorcode", "__dict__", "__annotations__", "__spec__"]:
        try:
            print(getattr(errno, name) in reverse)
        except TypeError as exc:
            print(str(exc))
            print(repr(exc))
    for sample in ["", "!?A", "A1", "ǅ", "Aǅ", "Aʕ", "A꟎", "A𖺠"]:
        print(sample.isupper())


main()
