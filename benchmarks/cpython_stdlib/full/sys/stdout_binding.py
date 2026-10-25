"""A saved stream remains usable independently of the sys namespace entry."""
import io
import sys
from sys import stdout as imported


def change_during_argument() -> str:
    sys.stdout = None
    return "receiver evaluated before argument\n"


def main() -> None:
    saved = sys.stdout
    print(saved.fileno())
    with open(sys.argv[1], "w", encoding="utf-8") as target:
        sys.stdout = target
        saved.write(str(sys.stdout.fileno() == target.fileno()) + "\n")
        print("redirected")
        sys.__displayhook__("é")
        imported.write("original\n")
        sys.stdout.write(change_during_argument())
        print("ignored while stdout is None")
        try:
            sys.stdout.write("unreachable")  # ty: ignore[unresolved-attribute]
        except AttributeError as error:
            saved.write(str(error) + "\n")
        del sys.stdout
        try:
            print("unreachable")
        except RuntimeError as error:
            saved.write(str(error) + "\n")
        try:
            del sys.stdout
        except AttributeError as error:
            saved.write(str(error) + "\n")
        sys.stdout = saved
    capture = io.StringIO()
    sys.stdout = capture
    try:
        print("雪")
        print(0.5)
        print(b"\xff")
        sys.__displayhook__("é")
        sys.stdout.write("end")
        content = capture.getvalue()
        capture.close()
        try:
            sys.__displayhook__(42)
        except ValueError as error:
            saved.write(str(error) + "\n")
    finally:
        sys.stdout = saved
    print(repr(content))
    print("restored")


main()
