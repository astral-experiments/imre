import logging


def render(fmt: str, value: str) -> None:
    try:
        print(repr(fmt % value))
    except TypeError as error:
        print("TypeError: " + str(error))
    except ValueError as error:
        print("ValueError: " + str(error))


def format_source() -> str:
    print("format")
    return "%r"


def operand() -> str:
    print("argument")
    return "retained"


# Literal patterns must follow the same conversions and runtime errors as
# formats read from variables.
def literal_formats(value: str) -> None:
    print("Log file %r does not exist" % value)
    print("errno %s / %% done" % value)
    print("%8.2a" % value)
    try:
        print("invalid %" % value)
    except ValueError as error:
        print(str(error))
    try:
        print("missing %s %s" % value)
    except TypeError as error:
        print(str(error))


for fmt in ["file %r is missing", "%%%-7.2s/%%", "%#05s", "%a", "%.2r", "%c",
            "%ld", "%i", "%#o", "%X", "%g", "%(name)s", "%(name", "%*s", "%.*s",
            "%3*s", "%.2*s", "%s %s", "%s %.", "π%界", "%hhs", "%3%", "%s %%", "plain",
            "%9999999999999999999999s", "%.9999999999999999999999s"]:
    for value in ["", "π", "💙a", "a'\n\x00"]:
        render(fmt, value)
print(format_source() % operand())
for fmt, n in [("%*s", 3), ("%.*r", 2**40), ("%*d", 2**65), ("%*", 2),
               ("%g", 2**1024), ("%.1e", -(2**1024)), ("%g", 2**1024 - 2**970 - 1)]:
    try:
        print(fmt % n)
    except Exception as error:
        print(repr(error).split("(", 1)[0])
record = logging.LogRecord("app", 20, "mod.py", 1, "[%*.*s] %d", (-6, 2, "λ🙂x", 3), None)
print(record.getMessage())
record.args = {"()": "yes"}
for msg in ["%(())s", "%(()", "%(missing(x))s", "%(())s/%s", "%s/%(())s"]:
    record.msg = msg
    try:
        print(record.getMessage())
    except KeyError as error:
        print(repr(error))
    except Exception as error:
        print(repr(error).split("(", 1)[0])
formatter = logging.Formatter("%(message)s%(())s")
print(formatter.format(logging.makeLogRecord({"()": "yes"})))

literal_formats("a'π")
literal_formats(operand())
