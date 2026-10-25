"""Compare handler ordering, identity, level filtering, flush and the CP1251 terminator."""
import codecs
import io
import logging
import sys


def handle_result(handler: logging.Handler, record: logging.LogRecord) -> None:
    rv = handler.handle(record)
    if rv is None:
        print("None")
    elif isinstance(rv, bool):
        print(rv)
    else:
        print(rv is record)


def exercise() -> None:
    child = logging.getLogger("delivery.child")
    parent = logging.getLogger("delivery")
    parent.propagate = False
    parent.setLevel(logging.DEBUG)
    output = io.StringIO()
    first = logging.StreamHandler(output)
    second = logging.StreamHandler(output)
    first.setFormatter(logging.Formatter("child:%(message)s"))
    second.setFormatter(logging.Formatter("%(name)s:%(levelname)s:%(message)s"))
    first.terminator = "|"
    parent.addHandler(second)
    saved = child.handlers
    child.addHandler(first)
    child.addHandler(first)
    print(len(saved))
    print(child.hasHandlers())
    print(len(parent.handlers))
    child.warning("hello")
    first.setLevel(logging.ERROR)
    child.warning("parent only")
    child.removeHandler(first)
    child.removeHandler(first)
    print(len(saved))
    print(child.hasHandlers())
    child.propagate = False
    print(child.hasHandlers())
    print(output.getvalue())
    record = logging.LogRecord("delivery.child", 30, "source.py", 7, "checked", (), None)
    handle_result(first, record)
    first.filters.append(logging.Filter("different"))
    handle_result(first, record)
    handle_result(logging.NullHandler(), record)
    binary = io.BytesIO()
    writer = codecs.getwriter("cp1251")(binary)
    handler = logging.StreamHandler(writer)
    child.addHandler(handler)
    handler.terminator = ""
    child.warning("до свидания")
    incomplete = binary.getvalue()
    complete = b"\xe4\xee \xf1\xe2\xe8\xe4\xe0\xed\xe8\xff\n"
    assert incomplete == complete[:-1]
    try:
        if incomplete != complete:
            raise AssertionError("missing newline")
    except AssertionError as exc:
        print(str(exc))
    else:
        raise RuntimeError("equality accepted a missing newline")
    handler.terminator = "\n"
    child.warning("до свидания")
    print(binary.getvalue() == incomplete + complete)
    handler.close()
    print(binary.closed)
    child.removeHandler(handler)
    parent.removeHandler(second)
    first.close()
    second.close()

    handler = logging.StreamHandler()
    one = io.StringIO()
    two = io.StringIO()
    original = handler.setStream(one)
    print(original is sys.stderr)
    previous = handler.setStream(two)
    print(previous is one)
    print(previous is two)
    print(handler.setStream(two) is None)
    previous = handler.setStream(None)
    print(previous is two)
    handler.flush()
    print(handler.setStream(original) is None)
    handler.close()


exercise()
