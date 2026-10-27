"""Observe individual writes and the lifetime of the stream selected by print."""

import sys
from io import StringIO

events = []


class Sink:
    def __init__(self, label):
        self.label = label

    @property
    def write(self):
        events.append(("lookup", self.label))
        return self.record

    def record(self, text):
        events.append((self.label, text))

    def flush(self):
        events.append(("flush", self.label))


class Replace:
    def __str__(self):
        events.append("str")
        sys.stdout = Sink("new")
        for index in range(6000):
            temporary = [index]
        return "first"


def current():
    original = sys.stdout
    sys.stdout = Sink("old")
    print(Replace(), "next", sep="|", end="!", flush=True)
    print("second", end="")
    sys.stdout = original
    print(events)
    capture = StringIO()
    print(None, "é", file=capture, sep=None, end=None)
    print(repr(capture.getvalue()))
    sys.stdout = None
    print("discarded", sep=1)
    sys.stdout = original
    for keywords in ({"sep": 1}, {"end": []}, {"file": object()}):
        try:
            print("invalid", **keywords)
        except (TypeError, AttributeError) as error:
            print(type(error).__name__, str(error))


if __name__ == "__main__":
    current()
