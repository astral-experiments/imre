"""Read saved stream owners and exercise the original builtin display hook."""
import io
import sys


def mark(label: str, value: str | None) -> str | None:
    sys.__stderr__.write(label)
    return value


def addition(left: str | None, right: str | None) -> None:
    try:
        value = mark("left|", left) + mark("right|", right)
        sys.__stderr__.write(value + "\n")
    except TypeError as error:
        sys.__stderr__.write(str(error) + "\n")


def main() -> None:
    original = sys.stdout
    errors = original.errors
    encoding = original.encoding
    sys.__stderr__.write(encoding + "|" + errors + "\n")
    capture = io.StringIO()
    sys.stdout = capture
    print("captured café")
    sys.__displayhook__("café")
    sys.__stderr__.write(str(sys.stdout.errors is None) + "\n")
    sys.__stderr__.write(str(sys.stdout.encoding is None) + "\n")
    try:
        original.write("saved café€|")
    except UnicodeEncodeError as error:
        sys.__stderr__.write(str(error) + "\n")
    except LookupError as error:
        sys.__stderr__.write(str(error) + "\n")
    sys.__stderr__.write(str(sys.__stdout__.encoding is encoding) + "\n")
    sys.__stderr__.write(str(sys.__stdout__.errors is errors) + "\n")
    sys.__stderr__.write(capture.getvalue())
    sys.stdout = original
    original.write("pending|")
    try:
        sys.__displayhook__("café€")
    except LookupError as error:
        sys.__stderr__.write(str(error) + "\n")
    print("tail")
    sys.stdout.flush()
    addition("a", "b")
    addition(None, "b")
    addition("a", None)
    addition(None, None)


main()
