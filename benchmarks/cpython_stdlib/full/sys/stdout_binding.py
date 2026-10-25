"""A saved stream remains usable independently of the sys namespace entry."""
import sys
from sys import stdout as imported


def change_during_argument() -> str:
    sys.stdout = None
    return "receiver evaluated before argument\n"


def main() -> None:
    saved = sys.stdout
    with open(sys.argv[1], "w", encoding="utf-8") as target:
        sys.stdout = target
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
    print("restored")


main()
