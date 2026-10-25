"""Read retained text wrapper metadata through the live stdout binding."""
import io
import sys
from pathlib import Path


def current() -> str | None:
    return sys.stdout.encoding


def report(value: str | None) -> None:
    if value is None:
        sys.__stdout__.write("None\n")
    else:
        sys.__stdout__.write(value + "\n")


def check(value: bool) -> None:
    sys.__stdout__.write(str(value) + "\n")


def main() -> None:
    original = sys.stdout
    report(current())
    check(current() is sys.__stdout__.encoding)
    check(sys.__stdout__.encoding is not sys.__stderr__.encoding)
    capture = io.StringIO()
    sys.stdout = capture
    report(current())
    capture.close()
    report(current())
    spelling = "utf-8"
    with open(sys.argv[1], "w", encoding="utf-8") as explicit:
        sys.stdout = explicit
        name = current()
        report(name)
        check(name is explicit.encoding)
        check(name is not spelling)
        print("written to disk")
    check(name is explicit.encoding)
    report(current())
    with Path(sys.argv[1]).open(encoding="utf-8") as reader:
        report(reader.encoding)
        check(reader.encoding is not name)
    with open(sys.argv[1]) as default_file:
        sys.stdout = default_file
        report(current())
    try:
        open(sys.argv[1] + ".missing", encoding="utf-8")
    except FileNotFoundError:
        report("missing explicit-encoding file")
    sys.stdout = None
    try:
        report(current())
    except AttributeError as error:
        report(str(error))
    del sys.stdout
    try:
        report(current())
    except AttributeError as error:
        report(str(error))
    sys.stdout = original
    check(current() is original.encoding)


main()
