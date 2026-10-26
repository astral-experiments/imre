"""Full logging configuration closes live handlers, including unattached ones."""
import io
import logging
import logging.config
from pathlib import Path


class Tracked(logging.Handler):
    tag: str
    events: list[str]

    def __init__(self, tag: str, events: list[str]) -> None:
        super().__init__()
        self.tag = tag
        self.events = events

    def flush(self) -> None:
        self.events.append("flush " + self.tag)

    def close(self) -> None:
        self.events.append("close " + self.tag)
        super().close()


def exercise() -> None:
    sink = io.StringIO()
    named = logging.StreamHandler(sink)
    named.name = "saved"
    path = Path("sink.log")
    file = logging.FileHandler(path, mode="w", encoding="utf-8")
    stream = file.stream
    events: list[str] = []
    first = Tracked("first", events)
    second = Tracked("second", events)
    try:
        logging.config.dictConfig({"version": 2})
    except ValueError:
        print(logging.getHandlerByName("saved") is named)
    logging.config.dictConfig({"version": 1, "incremental": True})
    print(file.stream is stream)
    try:
        logging.config.dictConfig({"version": 1, "root": {"level": "INVALID"}})
    except ValueError:
        print(logging.getHandlerByName("saved") is None)
    print(sink.closed)
    print(file.stream is None)
    if stream is not None:
        print(stream.closed)
    print(first._closed)
    print(second._closed)
    print(events)
    logging.config.dictConfig({"version": 1})
    print(events)
    fresh = logging.StreamHandler(sink)
    fresh.name = "fresh"
    config = Path("logging.ini")
    config.write_text("[loggers]\nkeys=root\n", encoding="utf-8")
    try:
        logging.config.fileConfig(str(config))
    except KeyError:
        print(logging.getHandlerByName("fresh") is fresh)
    config.write_text("[formatters]\nkeys=\n[handlers]\nkeys=\n[loggers]\nkeys=root\n"
                      "[logger_root]\nlevel=WARNING\nhandlers=\n", encoding="utf-8")
    logging.config.fileConfig(str(config))
    print(logging.getHandlerByName("fresh") is None)
    print(fresh._closed)


exercise()
