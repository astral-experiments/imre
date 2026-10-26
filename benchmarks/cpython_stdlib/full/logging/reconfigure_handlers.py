import io
import logging
import logging.config
import sys


def main() -> None:
    root = logging.getLogger()
    parent = logging.getLogger("cfg.parent")
    child = logging.getLogger("cfg.parent.child")
    other = logging.getLogger("separate")
    handler = logging.StreamHandler(io.StringIO())
    root.addHandler(handler)
    parent.addHandler(handler)
    child.addHandler(handler)
    other.addHandler(handler)
    child.setLevel(logging.ERROR)
    child.propagate = False
    roots = root.handlers
    parents = parent.handlers
    children = child.handlers
    others = other.handlers
    standalone = logging.Logger("cfg.parent.child")
    standalone.addHandler(handler)
    independent = standalone.handlers
    assert independent is not children

    logging.config.dictConfig({"version": 1, "incremental": True,
                               "root": {"level": "INFO"}, "loggers": {"cfg.parent": {"level": "INFO"}}})
    print(roots is root.handlers and parents is parent.handlers and children is child.handlers)
    print(len(roots) == 1 and len(parents) == 1 and len(children) == 1)
    try:
        logging.config.dictConfig({"version": 1, "disable_existing_loggers": False,
                                   "root": {"level": "invalid"}})
    except ValueError:
        print(roots is root.handlers and len(roots) == 1)

    if sys.argv[1] == "file":
        with open(sys.argv[2], "w") as config:
            config.write("[loggers]\nkeys=root,parent\n[handlers]\nkeys=\n[formatters]\nkeys=\n"
                         "[logger_root]\nlevel=WARNING\nhandlers=\n[logger_parent]\nqualname=cfg.parent\n"
                         "level=ERROR\nhandlers=\npropagate=0\n")
        logging.config.fileConfig(sys.argv[2], disable_existing_loggers=False)
    else:
        logging.config.dictConfig({"version": 1, "disable_existing_loggers": False,
                                   "loggers": {"cfg.parent": {"level": "ERROR", "propagate": False}},
                                   "root": {"level": "WARNING"}})
    assert standalone.handlers is independent
    assert len(independent) == 1 and independent[0] is handler
    print(roots is root.handlers)
    print(len(roots))
    print(parents is parent.handlers)
    print(len(parents))
    print(children is child.handlers)
    print(len(children))
    print(children[0] is handler)
    print(len(child.handlers))
    print(child.level)
    print(child.propagate)
    print(others is other.handlers and len(others) == 1 and others[0] is handler)
    print(root.hasHandlers())
    print(parent.hasHandlers())
    print(child.hasHandlers())
    print(other.hasHandlers())


main()
