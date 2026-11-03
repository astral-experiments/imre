"""Explicit shutdown visits live handlers again and respects each handler's flushOnClose."""
import logging


class Recorder(logging.Handler):
    events: list[str]
    label: str
    flushOnClose: bool

    def __init__(self, label: str, flush: bool, events: list[str]) -> None:
        super().__init__()
        self.label = label
        self.flushOnClose = flush
        self.events = events

    def flush(self) -> None:
        self.events.append("flush " + self.label)

    def close(self) -> None:
        self.events.append("close " + self.label)


def exercise() -> None:
    events: list[str] = []
    first = Recorder("first", False, events)
    Recorder("discarded", True, events)
    last = Recorder("last", True, events)
    for turn in range(2):
        assert logging.shutdown() is None
    assert events == ["flush last", "close last", "close first"] * 2
    assert first.level == last.level


exercise()
