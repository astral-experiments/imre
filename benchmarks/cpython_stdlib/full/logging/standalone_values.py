import logging


def argument(label: str, value: str) -> str:
    print(label)
    return value


def main() -> None:
    logging.basicConfig(level=logging.DEBUG, format="root:%(message)s")
    a = logging.Logger("branch")
    b = logging.Logger(name="branch", level="ERROR")
    print(a is b, a.level, b.level, a.getEffectiveLevel(), b.getEffectiveLevel())
    print(a.parent is None, b.parent is None, a.root is logging.root)
    print(a.hasHandlers(), b.hasHandlers(), logging.root.hasHandlers())
    print(a._cache is b._cache, a.isEnabledFor(10))
    held = a._cache
    registered = logging.getLogger("branch")
    child = logging.getLogger("branch.child")
    print(a is registered, registered is logging.getLogger("branch"), child.parent is registered)
    print(len(a.getChildren()), child in registered.getChildren(), registered in logging.root.getChildren())
    print(a.parent is None, registered.parent is logging.root, registered.hasHandlers())
    a.propagate = False
    print(a.propagate, b.propagate, registered.propagate)
    print(registered.isEnabledFor(10), b.isEnabledFor(10))
    a.setLevel(logging.ERROR)
    print(a.level, a.getEffectiveLevel(), len(registered._cache))
    print(a.isEnabledFor(10), a.isEnabledFor(11), held is a._cache, held[10], held[11])
    logging.disable(100)
    print(a.isEnabledFor(10), a.isEnabledFor(41), registered.isEnabledFor(41))
    logging.disable(logging.NOTSET)
    print(a.isEnabledFor(41), a.isEnabledFor(42), registered.isEnabledFor(41))
    a.error("direct error")
    registered.warning("registered warning")

    root_name = logging.Logger("root")
    empty = logging.Logger("")
    print(root_name is logging.root, root_name.name, root_name.parent is None, root_name.hasHandlers())
    print(empty.name == "", empty.parent is None, empty.getEffectiveLevel(), empty.hasHandlers())
    logging.addLevelName(70, "CUSTOM")
    print(logging.Logger("named level", "CUSTOM").level)
    try:
        logging.Logger(level=argument("level evaluated", "unknown"), name=argument("name evaluated", "invalid"))
    except ValueError as exc:
        print(str(exc))
    try:
        logging.Logger("invalid", None)
    except TypeError as exc:
        print(str(exc))


main()
