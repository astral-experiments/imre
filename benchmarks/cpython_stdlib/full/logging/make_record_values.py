"""Compare protected extras and live record dictionary aliases."""
import logging


def value(tag: str, v: str) -> str:
    print(tag)
    return v


class Marker:
    pass


def retain(record: logging.LogRecord, when: float | None, arg: str | bytes) -> None:
    fields = record.__dict__
    fields["nullable"] = when
    fields["choice"] = arg
    print(str(fields["nullable"]))
    print(bool(fields["nullable"]))
    print(fields["nullable"] == 0)
    print(str(fields["choice"]))
    print(bool(fields["choice"]))
    print(fields["choice"] == b"")


def size(record: logging.LogRecord, key: str) -> int:
    return len(record.__dict__[key])


def first(record: logging.LogRecord, key: str) -> str:
    return str(record.__dict__[key][0])


def check() -> None:
    logger = logging.getLogger("factory")
    extra = {"event": "first", "other": "second"}
    record = logger.makeRecord("my record", 13, "folder/file.py", 9, "value %s", ("one",),
                               None, func="sender", extra=extra)
    fields = record.__dict__
    print(fields is record.__dict__)
    print(list(fields))
    print("event" in fields)
    print(logging.Formatter("%(name)s %(event)s %(message)s").format(record))
    extra["event"] = "changed in source"
    print(str(fields["event"]))
    fields["event"] = fields["other"]
    print(logging.Formatter("%(event)s").format(record))
    print(str(fields["message"]))
    del fields["msg"]
    try:
        print(record.getMessage())
    except AttributeError as exc:
        print(str(exc))
    fields["msg"] = fields["event"]
    fields["args"] = fields["exc_info"]
    print(record.getMessage())
    for key in ["message", "asctime", "name", "args", "thread", "process", "taskName"]:
        try:
            logger.makeRecord("r", 30, "", 1, "m", (), None, extra={key: "clash"})
        except KeyError as exc:
            print(str(exc))
    other = logger.makeRecord(msg=value("message first", "hello"), args=(), exc_info=None,
                              level=20, name=value("name second", "r"), lno=2,
                              fn=value("path third", ""), extra=None)
    print(other.getMessage())
    print("event" in other.__dict__)
    restored = logging.makeLogRecord({"name": "allowed", "message": "allowed message"})
    print(restored.name == "allowed")
    print(str(restored.__dict__["message"]))

    copied = logging.makeLogRecord(fields)
    print(len(copied.__dict__) == len(fields))
    for name in fields:
        assert copied.__dict__[name] == fields[name]
    fields["msg"] = "replaced through dict"
    fields["created"] = 0.0
    print(record.getMessage())
    print(copied.getMessage())
    print(repr(fields["created"]))
    print(bool(fields["created"]))
    try:
        print(len(fields["created"]))
    except TypeError as exc:
        print(str(exc))
    fields["created"] = -2.5
    print(str(fields["created"]))
    print(bool(fields["created"]))
    values = ["first", "second"]
    fields["values"] = values
    print(len(fields["values"]))
    print(str(fields["values"][-1]))
    values.append("third")
    print(repr(fields["values"]))
    fields["by_number"] = {7: "seven"}
    print(len(fields["by_number"]))
    print(str(fields["by_number"][7]))
    try:
        print(str(fields["by_number"][-1]))
    except KeyError as exc:
        print(str(exc))
    fields["by_name"] = {"one": 1}
    try:
        print(str(fields["by_name"][1]))
    except KeyError as exc:
        print(str(exc))
    fields["items"] = {"set item"}
    print(bool(fields["items"]))
    print(len(fields["items"]))
    try:
        print(first(record, "items"))
    except TypeError as exc:
        print(str(exc))

    retain(record, None, "zero")
    retain(record, 0.0, b"")
    print(fields["nullable"] == 0.0)
    print(fields["by_number"] == {7: "seven"})
    marker = Marker()
    fields["marker"] = marker
    print(fields["marker"] == marker)
    print(bool(fields["marker"]))
    try:
        print(size(record, "marker"))
    except TypeError as exc:
        print(str(exc))


check()
