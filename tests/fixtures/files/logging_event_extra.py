"""Compare ordered call arguments, retained extras, collisions and delivery in one process."""
import logging


class Capture(logging.Handler):
    records: list[logging.LogRecord]

    def __init__(self) -> None:
        super().__init__()
        self.records = []

    def emit(self, record: logging.LogRecord | None) -> None:
        if record is not None:
            self.records.append(record)


def text(label: str, obj: str) -> str:
    print(label)
    return obj


def context(label: str, key: str) -> dict[str, str]:
    print(label)
    return {key: label}


def level(label: str) -> int:
    print(label)
    return logging.INFO


def optional(logger: logging.Logger, fields: dict[str, str] | None) -> None:
    logger.info("optional", extra=fields)


def check() -> None:
    logger = logging.getLogger("event.child")
    parent = logging.getLogger("event")
    parent.propagate = False
    first = Capture()
    second = Capture()
    logger.addHandler(first)
    parent.addHandler(second)
    logger.setLevel(logging.WARNING)
    logger.log(level("disabled level"), text("disabled message", "%s"),
               text("disabled argument", "ignored"), extra=context("disabled extra", "msg"))
    print(len(first.records))
    logger.setLevel(logging.DEBUG)
    logger.log(level("enabled level"), text("enabled message", "value %s"),
               text("enabled argument", "kept"), extra=context("enabled extra", "tag"))
    record = first.records[0]
    print(record is second.records[0])
    print(record.getMessage())
    print(str(record.__dict__["tag"]))
    print(logging.Formatter("%(tag)s: %(message)s").format(record))
    logger.info("mixed literal", extra={"event": "notice", "number": 12, "opt": None})
    fields = first.records[-1].__dict__
    print(str(fields["event"]))
    print(str(fields["number"]))
    print(str(fields["opt"]))
    source = {"old": "value"}
    logger.warning("copied", extra=source)
    source["old"] = "changed"
    print(str(first.records[-1].__dict__["old"]))
    optional(logger, None)
    print("old" in first.records[-1].__dict__)
    optional(logger, source)
    print(str(first.records[-1].__dict__["old"]))
    logger.error("empty", extra={})
    print(first.records[-1].getMessage())
    before = len(first.records)
    for key in ["asctime", "message", "msg", "name", "args"]:
        try:
            logger.critical("bad", extra={"allowed": "first", key: "no"})
        except KeyError as exc:
            print(str(exc))
    print(before == len(first.records))
    print(len(first.records) == len(second.records))
    print(len(logging.root.handlers))
    try:
        logging.warning("root bad", extra={"name": "clash"})
    except KeyError as exc:
        print(str(exc))
    print(len(logging.root.handlers))
    root_handler = Capture()
    logging.basicConfig(handlers=[root_handler], level=logging.DEBUG, force=True)
    logging.debug("debug", extra={"with": "debug"})
    logging.info("info", extra={"with": "info"})
    logging.warning("warning", extra={"with": "warning"})
    logging.error("error", extra={"with": "error"})
    logging.critical("critical", extra={"with": "critical"})
    logging.log(logging.WARNING, "log", extra={"with": "log"})
    for item in root_handler.records:
        print(logging.Formatter("%(levelname)s %(with)s %(message)s").format(item))


check()
