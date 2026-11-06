import gc
import io
import re


def current():
    events = []

    class Items(list):
        def count(self, value):
            return ("override", value)

    class Descriptor:
        @property
        def count(self):
            events.append("lookup")
            return lambda value: ("called", value)

    def argument():
        events.append("argument")
        return 3

    assert Items().count(4) == ("override", 4)
    assert Descriptor().count(argument()) == ("called", 3)
    assert events == ["lookup", "argument"]
    try:
        object().missing(argument())
    except AttributeError:
        assert events == ["lookup", "argument"]

    class Element:
        def __del__(self):
            events.append("finalized")

    owners = [[Element()]]

    def drop_owner():
        owners.clear()
        gc.collect()
        assert "finalized" not in events
        return 0

    assert owners[0].count(drop_owner()) == 0
    assert events[-1] == "finalized"
    assert (1, 2, 1).count(1) == 2

    class Writer:
        def write(self, value):
            return ("original", value)

    writers = [Writer()]

    def replace_writer():
        Writer.write = lambda self, value: ("replacement", value)
        writers.clear()
        gc.collect()
        return 7

    assert writers[0].write(replace_writer()) == ("original", 7)

    with io.TextIOWrapper(io.BytesIO(), encoding="utf-8") as stream:
        def shadow_write():
            stream.write = lambda text: 0
            return "x"

        assert stream.write(shadow_write()) == 1
        assert stream.write("second") == 0
        del stream.write
        assert stream.write("y") == 1
        stream.flush()
        assert stream.buffer.getvalue() == b"xy"

    pattern = re.compile(r"(a)(b)?")
    match = pattern.search("--ab")
    assert match.span() == (2, 4) and match.group(2) == "b"
    scanner = pattern.scanner("aab")
    assert scanner.search().group(0) == "a" and scanner.match().span() == (1, 3)
