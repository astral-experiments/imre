"""Compare name bindings, lifetimes, and base Handler behavior in one process."""
import logging


def temporary() -> None:
    ephemeral = logging.Handler()
    ephemeral.name = "temporary"
    print(logging.getHandlerByName("temporary") is ephemeral)


first = logging.Handler("INFO")
second = logging.Handler()
print(first.name is None)
print(first.level)
first.name = "shared"
second.name = "shared"
print(logging.getHandlerByName("shared") is second)
first.set_name("renamed")
print(logging.getHandlerByName("shared") is None)
print(logging.getHandlerByName("renamed") is first)
print(second.get_name() == "shared")
saved = logging.getHandlerByName("renamed")
first.close()
print(first.name == "renamed")
print(first._closed)
print(logging.getHandlerByName("renamed") is None)
print(saved is first)
first.name = "new"
print(logging.getHandlerByName("new") is first)
first.name = ""
print(logging.getHandlerByName("") is None)
first.name = None
temporary()
print(logging.getHandlerByName("temporary") is None)
logging.addLevelName(19, "CUSTOM")
second.setLevel("CUSTOM")
print(second.level)
try:
    second.setLevel("not a level")
except ValueError as exc:
    print(str(exc))
print(second.level)
record = logging.makeLogRecord({"msg": "message"})
print(second.format(record))
second.setFormatter(logging.Formatter("[%(message)s]"))
print(second.format(record))
second.setFormatter(None)
print(second.format(record))
try:
    first.emit(None)
except NotImplementedError as exc:
    print(str(exc))
first.close()
second.close()
