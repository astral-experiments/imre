import io
import logging
import logging.config


def value() -> bool:
    print("value")
    return True


def target() -> logging.Logger:
    print("target")
    return logging.getLogger("flag.parent")


def main() -> None:
    parent = logging.getLogger("flag.parent")
    child = logging.getLogger("flag.parent.child")
    detached = logging.Logger("flag.parent")
    parent.setLevel(logging.DEBUG)
    detached.setLevel(logging.DEBUG)
    sink = io.StringIO()
    parent.addHandler(logging.StreamHandler(sink))
    own_sink = io.StringIO()
    detached.addHandler(logging.StreamHandler(own_sink))

    cache = parent._cache
    print(parent.isEnabledFor(logging.WARNING))
    print(cache[logging.WARNING])
    target().disabled = value()
    print(parent.disabled, detached.disabled, child.disabled)
    print(parent.isEnabledFor(logging.WARNING), parent.isEnabledFor(logging.ERROR))
    print(cache is parent._cache, cache[logging.WARNING], logging.ERROR in cache)
    parent.warning("hidden parent")
    child.warning("child propagated through disabled parent")
    detached.warning("detached still enabled")
    print(sink.getvalue())
    print(own_sink.getvalue())

    detached.disabled = True
    parent.disabled = False
    print(parent.isEnabledFor(logging.WARNING), detached.isEnabledFor(logging.WARNING))
    detached.warning("hidden detached")
    parent.warning("parent enabled again")
    print(sink.getvalue())
    print(own_sink.getvalue())

    logging.config.dictConfig({"version": 1})
    print(parent.disabled, child.disabled, detached.disabled)
    logging.config.dictConfig({"version": 1, "incremental": True,
                               "loggers": {"flag.parent": {"level": "INFO"}}})
    print(parent.disabled, child.disabled, detached.disabled)


main()
