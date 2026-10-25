import logging


def target() -> logging.Manager:
    print("target")
    return logging.root.manager


def invalid_level() -> str:
    print("value")
    return "unregistered-level"


def main() -> None:
    root = logging.root
    manager = root.manager
    child = logging.getLogger("manager.child")
    detached = logging.Logger("manager.child")
    no_root = logging.Manager(None)
    separate = logging.Manager(rootnode=root)
    print(manager is child.manager)
    print(manager is detached.manager)
    print(manager is not separate)
    print(no_root is not separate)
    print(no_root.root is None)
    print(separate.root is root)
    print(manager.root is root)
    child.setLevel(logging.DEBUG)
    cache = child._cache
    print(child.isEnabledFor(22))
    manager.disable = 23
    print(root.manager.disable)
    print(child.isEnabledFor(22))
    print(child.isEnabledFor(23))
    print(child.isEnabledFor(24))
    print(cache)
    separate.disable = "ERROR"
    print(separate.disable)
    print(manager.disable)
    print(no_root.disable)
    logging.disable(23)
    print(cache is child._cache)
    print(len(cache))
    print(child.isEnabledFor(22))
    print(child.isEnabledFor(24))
    try:
        target().disable = invalid_level()
    except ValueError as error:
        print(str(error))
    print(manager.disable)
    try:
        manager.disable = None
    except TypeError as error:
        print(str(error))
    print(manager.disable)
    logging.addLevelName(97, "local-policy")
    manager.disable = "local-policy"
    print(manager.disable)
    logging.disable()
    print(manager.disable)
    print(separate.disable)
    print(no_root.disable)


main()
