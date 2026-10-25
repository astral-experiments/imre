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


for fmt in ["file %r is missing", "%%%-7.2s/%%", "%#05s", "%a", "%.2r", "%c",
            "%ld", "%i", "%#o", "%X", "%g", "%(name)s", "%(name", "%*s", "%.*s",
            "%3*s", "%.2*s", "%s %s", "%s %.", "π%界", "%hhs", "%3%", "%s %%", "plain",
            "%9999999999999999999999s", "%.9999999999999999999999s"]:
    for value in ["", "π", "💙a", "a'\n\x00"]:
        render(fmt, value)
print(format_source() % operand())
