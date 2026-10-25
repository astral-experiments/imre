"""Exercise the same public object view for Template values and an imported module."""
from string import Template
import errno


def render(item: object) -> str:
    return Template("$value").substitute(value=item)


def check(name: str, index: int) -> None:
    item = getattr(errno, name, ("missing", 2))
    print(name)
    print(isinstance(item, dict))
    print(isinstance(item, tuple))
    print(bool(item))
    print(item == item)
    print(item == errno.errorcode)
    print(errno.errorcode == item)
    try:
        print(item in errno.errorcode)
    except TypeError as exc:
        print(str(exc))
    try:
        print(len(item))
    except TypeError as exc:
        print(str(exc))
    try:
        print(repr(item[index]))
    except (TypeError, IndexError, KeyError) as exc:
        print(str(exc))


def mutate(key: str) -> None:
    alias = getattr(errno, key)
    errno.errorcode[123456] = "changed"
    print(alias[123456])
    del errno.errorcode[123456]
    try:
        print(alias[123456])
    except KeyError as exc:
        print(str(exc))



for key in ["ENOENT", "__name__", "errorcode", "__dict__", "__class__", "__spec__",
            "__loader__", "__annotations__", "__repr__", "__dir__", "unknown-example"]:
    check(key, errno.ENOENT)
for key in ["ENOENT", "__name__", "__class__", "__spec__", "__loader__", "unknown-example"]:
    print(render(getattr(errno, key, ("missing", 2))))


mutate("errorcode")
