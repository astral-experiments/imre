"""Exercise retained native record fields and compare the output with the pinned interpreter."""
import logging


def record_values() -> None:
    print(str(logging.makeLogRecord({})))
    normal = logging.LogRecord("app", 20, "/path/to/.module.ext", 9, "%d %s", (4, "days"), None)
    print(normal.getMessage())
    print(normal.filename)
    print(normal.module)
    print(normal.levelname)
    logging.addLevelName(39, "CUSTOM")
    other = logging.LogRecord("app", 39, "f.py", 1, "", (), None)
    print(other.levelname)
    copy = normal
    normal.msg = "%(n)03d/%(n)#x"
    args = {"n": 7}
    normal.args = args
    args["n"] += 1
    print(copy.getMessage())
    normal.msg = "[%-5.2s] %r %c %.0f"
    normal.args = ("été", "name", 0x1F30D, 2.5)
    print(normal.getMessage())
    normal.msg = "%d:%d"
    normal.args = (10,)
    try:
        print(normal.getMessage())
    except TypeError as error:
        print(str(error))

    dynamic = {"new": "before", "msg": "sent"}
    received = logging.makeLogRecord(dynamic)
    dynamic["new"] = "after"
    retained: str = received.new
    print(retained)
    print(received.getMessage())
    r = logging.makeLogRecord({"args": (2, "placeholders"), "level": 10,
                               "msg": "Message with %d %s", "custom": 1234})
    custom: int = r.custom
    print(custom)
    print(r.getMessage())
    print(r.levelname)
    print(r.levelno is None)
    before = logging.makeLogRecord({})
    logging.logThreads = False
    after = logging.makeLogRecord({})
    print(before.thread is not None)
    print(after.thread is None)
    logging.logThreads = True


record_values()
