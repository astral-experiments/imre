"""Check omitted versus explicit lines, cache aliasing and default stream writes."""
import linecache
import sys
import warnings
from io import StringIO


class Message(ValueError):
    def __init__(self, detail: str) -> None:
        self.detail = detail
        super().__init__(detail)

    def __str__(self) -> str:
        if self.detail == "broken":
            raise OSError("message text unavailable")
        return "reported: " + self.detail


def optional_line(override: bool) -> str | None:
    if override:
        return " override "
    return None


def mark(text: str) -> str:
    print(text)
    return text


def write_optional(target: StringIO | None, path: str) -> None:
    warnings.showwarning("optional file", Warning, path, 2, target, "")


def main() -> None:
    source = sys.argv[1]
    dest = sys.argv[2]
    category = UserWarning
    print(repr(warnings.formatwarning("α", category, source, 1)))
    print(repr(warnings.formatwarning("α", category, source, 2, optional_line(False))))
    print(repr(warnings.formatwarning("α", category, source, 2, optional_line(True))))
    for line in ["", "  ", "\x1c", "  fresh\n", "have\na break "]:
        print(repr(warnings.formatwarning("m\nn", RuntimeWarning, source, 2, line)))
    print(repr(warnings.formatwarning("missing", ResourceWarning, source + ".missing", 2)))
    print(repr(warnings.formatwarning("huge", Warning, source, 1 << 100)))
    print(repr(warnings.formatwarning("bool", DeprecationWarning, source, True)))
    for value in [10**4300 - 1, 10**4300, -(10**4300)]:
        try:
            print(len(warnings.formatwarning(value, Warning, source, 1, "")))
        except ValueError as exc:
            print(str(exc))
        try:
            print(len(warnings.formatwarning("m", Warning, source, value, "")))
        except ValueError as exc:
            print(str(exc))
    try:
        warnings.showwarning(Message("broken"), Warning, source, 10**4300, line="")
    except ValueError as exc:
        print(str(exc))
    warnings.formatwarning(line=mark("line"), filename=mark(source), lineno=2,
                           message=mark("message"), category=category)
    lines = linecache.getlines(source)
    lines[0] = "  changed in cache\n"
    print(repr(warnings.formatwarning("cached", category, source, 1)))
    sink = StringIO()
    warnings.showwarning("first", category, source, 1, sink)
    warnings.showwarning("second", category, source, 2, sink, " explicit ")
    warnings.showwarning(Message("ok"), category, source, 2, sink, "")
    try:
        warnings.showwarning(Message("broken"), category, source, 2, sink, "")
    except OSError as error:
        print("formatting failed: " + str(error))
    write_optional(sink, source)
    write_optional(None, source)
    print(repr(sink.getvalue()))
    sink.close()
    try:
        warnings.showwarning("closed", Warning, source, 2, sink)
    except ValueError as error:
        print("closed: " + str(error))
    with open(source) as read_only:
        print(warnings.showwarning("unwritable", Warning, source, 1, read_only))
    with open(dest, "w") as output:
        warnings.showwarning("file", category, source, 2, output)
    with open(dest) as result:
        print(repr(result.read()))
    warnings.showwarning("stderr", Warning, source, 1)
    warnings.showwarning("explicit stderr", Warning, source, 2, sys.stderr, "")
    warnings.showwarning("stdout", Warning, source, 1, sys.stdout, "")


main()
