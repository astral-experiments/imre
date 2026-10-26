"""Compare adapter context, caller identity and hook ordering against CPython."""
import logging


class Capture(logging.Handler):
    records: list[logging.LogRecord]

    def __init__(self) -> None:
        super().__init__()
        self.records = []

    def emit(self, record: logging.LogRecord | None) -> None:
        if record is not None:
            self.records.append(record)


class Hooks(logging.LoggerAdapter):
    def isEnabledFor(self, level: int) -> bool:
        print("checking")
        return self.logger.isEnabledFor(level)

    def process(self, msg, kwargs):
        print("processing")
        print("extra" in kwargs)
        if msg == "switch %s":
            self.logger = logging.getLogger("adapter.second")
        if msg == "mute":
            self.logger.setLevel(logging.CRITICAL)
        if msg == "raise":
            raise ValueError("from process")
        return "processed " + msg, kwargs


def text(label: str) -> str:
    print(label)
    return label


def fields(label: str) -> dict[str, str]:
    print(label)
    return {"foo": label}


def caller_field(value: str | None) -> str:
    if value is None:
        raise AssertionError("the emitted record lost its caller")
    return value


def invoke(adapter: logging.LoggerAdapter) -> None:
    adapter.info("switch %s", "argument", extra={"foo": "moved"})


def check() -> None:
    first = logging.getLogger("adapter.first")
    second = logging.getLogger("adapter.second")
    first.propagate = False
    second.propagate = False
    first.setLevel(logging.WARNING)
    second.setLevel(logging.DEBUG)
    one = Capture()
    two = Capture()
    first.addHandler(one)
    second.addHandler(two)
    defaults = {"foo": "default"}
    a = logging.LoggerAdapter(first, defaults)
    a.debug(text("disabled message"), text("disabled operand"), extra=fields("disabled extra"))
    a.warning("default", extra={"bar": "ignored"})
    print(one.records[0].foo)
    print(hasattr(one.records[0], "bar"))
    a = logging.LoggerAdapter(first, defaults, merge_extra=True)
    a.critical("merged", extra={"foo": "per event", "bar": "included"})
    print(one.records[-1].foo)
    print(one.records[-1].bar)
    a.error("default again", extra=None)
    print(one.records[-1].foo)
    defaults["foo"] = "changed"
    a.log(logging.ERROR, "%s/%s", "left", 3)
    print(one.records[-1].getMessage())
    print(one.records[-1].foo)
    hooked = Hooks(first)
    hooked.info(text("hook disabled"), text("hook disabled operand"), extra=fields("hook disabled extra"))
    first.setLevel(logging.INFO)
    invoke(hooked)
    record = two.records[-1]
    print(caller_field(record.name))
    print(caller_field(record.funcName))
    print(record.getMessage())
    print(record.foo)
    hooked.warning("no arguments")
    hooked.warning("explicit none", extra=None)
    print(two.records[-1].getMessage())
    hooked.error("mute")
    print(len(two.records))
    try:
        hooked.critical("raise")
    except ValueError as exc:
        print(str(exc))
    try:
        a.critical("collision", extra={"msg": "error"})
    except KeyError as exc:
        print(str(exc))
    print(len(one.records))


check()
