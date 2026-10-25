"""Observe the target errno table, its aliases, and the shared mutable dictionary."""
import errno
from errno import EDOM as DOMAIN, ERANGE, errorcode as reverse

def main() -> None:
    mod = errno
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


main()
