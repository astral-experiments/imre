"""Compare the live root handler, formatter, ignored reconfiguration and forced replacement."""
import io
import logging
import sys


def exercise() -> None:
    root = logging.getLogger("")
    root.setLevel(17)
    live = root.handlers
    logging.basicConfig()
    print(len(live))
    print(root.level)
    original = live[0]
    print(isinstance(original, logging.StreamHandler))
    root.removeHandler(original)
    original.close()
    old_sink = io.StringIO()
    new_sink = io.StringIO()
    old = logging.StreamHandler(old_sink)
    old.set_name("old")
    original_formatter = logging.Formatter("old:%(message)s")
    old.setFormatter(original_formatter)
    logging.basicConfig(handlers=[old], level="WARNING")
    logging.basicConfig(level="NOT_A_LEVEL", format="no variables")
    logging.warning("first")
    logging.info("suppressed")
    print(len(live))
    print(old_sink.getvalue())
    new = logging.StreamHandler(new_sink)
    new.set_name("new")
    logging.basicConfig(handlers=[new], force=True, level=logging.INFO, style="{")
    print(live[0] is new)
    print(old._closed)
    print(logging.getHandlerByName("old") is None)
    print(old_sink.closed)
    logging.warning("second")
    logging.info("third")
    print(new_sink.getvalue())
    print(old_sink.getvalue())
    try:
        logging.basicConfig(handlers=[old], stream=None, force=True)
    except ValueError as exc:
        print(str(exc))
    print(len(live))
    print(new._closed)
    explicit = io.StringIO()
    logging.basicConfig(format=None, stream=explicit, style="$")
    logging.error("just the message")
    print(explicit.getvalue())
    logging.basicConfig(force=True, handlers=[])
    print(len(live))
    print(explicit.closed)
    unattached = logging.StreamHandler(sys.stdout)
    try:
        value = unattached.formatter
        print(value.datefmt is None)
    except AttributeError as exc:
        print(str(exc))
    fmt = logging.Formatter(datefmt="bar")
    unattached.setFormatter(fmt)
    value = unattached.formatter
    date = value.datefmt
    assert date is not None
    print(date)


exercise()
