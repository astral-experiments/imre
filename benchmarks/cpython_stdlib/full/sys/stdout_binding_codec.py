"""Redirect, restore and use the original stream under different process codecs."""
import builtins
import io
import sys


def main() -> None:
    saved = sys.__stdout__
    text = "ab" + sys.argv[1]
    capture = io.StringIO()
    sys.stdout = capture
    print(text)
    sys.__displayhook__(text)
    saved.write(str(sys.stdout.encoding is None) + "|" + str(sys.stdout.errors is None) + "\n")
    content = capture.getvalue()
    sys.stdout = saved
    print(content == text + "\n" + repr(text) + "\n")
    try:
        saved.write(text)
    except UnicodeEncodeError as exc:
        sys.stderr.write(repr(exc.args) + "\n")
    try:
        print(text)
    except UnicodeEncodeError as exc:
        sys.stderr.write(repr(exc.args) + "\n")
    sys.__displayhook__(text)
    try:
        sys.__displayhook__(10**4400)
    except ValueError as exc:
        sys.stderr.write(str(exc) + "\n")
        print(builtins._ is None)
    sys.stdout = None
    try:
        sys.__displayhook__(10**4400)
    except RuntimeError as exc:
        saved.write(str(exc) + "\n")
    finally:
        sys.stdout = saved
    sys.__displayhook__(37)


main()
