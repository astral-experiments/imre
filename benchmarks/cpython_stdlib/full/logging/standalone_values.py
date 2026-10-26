import logging


def argument(label: str, value: str) -> str:
    print(label)
    return value


def main() -> None:
    logging.basicConfig(level=logging.DEBUG, format="root:%(message)s")
    a = logging.Logger("branch")
    b = logging.Logger(name="branch", level="ERROR")
    print(a is b)
    print(a.level)
    print(b.level)
    print(a.getEffectiveLevel())
    print(b.getEffectiveLevel())
    print(a.parent is None)
    print(b.parent is None)
    print(a.root is logging.root)
    print(a.hasHandlers())
    print(b.hasHandlers())
    print(logging.root.hasHandlers())
    print(a._cache is b._cache)
    print(a.isEnabledFor(10))
    held = a._cache
    registered = logging.getLogger("branch")
    child = logging.getLogger("branch.child")
    print(a is registered)
    print(registered is logging.getLogger("branch"))
    print(child.parent is registered)
    print(len(a.getChildren()))
    print(child in registered.getChildren())
    print(registered in logging.root.getChildren())
    print(a.parent is None)
    print(registered.parent is logging.root)
    print(registered.hasHandlers())
    a.propagate = False
    print(a.propagate)
    print(b.propagate)
    print(registered.propagate)
    print(registered.isEnabledFor(10))
    print(b.isEnabledFor(10))
    a.setLevel(logging.ERROR)
    print(a.level)
    print(a.getEffectiveLevel())
    print(len(registered._cache))
    print(a.isEnabledFor(10))
    print(a.isEnabledFor(11))
    print(held is a._cache)
    print(held[10])
    print(held[11])
    logging.disable(100)
    print(a.isEnabledFor(10))
    print(a.isEnabledFor(41))
    print(registered.isEnabledFor(41))
    logging.disable(logging.NOTSET)
    print(a.isEnabledFor(41))
    print(a.isEnabledFor(42))
    print(registered.isEnabledFor(41))
    a.error("direct error")
    registered.warning("registered warning")

    root_name = logging.Logger("root")
    empty = logging.Logger("")
    print(root_name is logging.root)
    print(root_name.name)
    print(root_name.parent is None)
    print(root_name.hasHandlers())
    print(empty.name == "")
    print(empty.parent is None)
    print(empty.getEffectiveLevel())
    print(empty.hasHandlers())
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
