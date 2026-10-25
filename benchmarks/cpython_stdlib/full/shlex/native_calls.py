"""Typed calls made on behalf of the original shlex tests."""
import shlex
import sys


def main() -> None:
    command = sys.argv[1]
    try:
        if command == "split":
            print(repr(shlex.split(sys.argv[2], comments=sys.argv[3] == "1",
                                   posix=sys.argv[4] == "1")))
        elif command == "split-none":
            print(repr(shlex.split(None)))
        elif command == "join":
            print(repr(shlex.join(sys.argv[2:])))
        elif command == "quote-str":
            print(repr(shlex.quote(sys.argv[2])))
        elif command == "quote-none":
            print(repr(shlex.quote(None)))
        elif command == "quote-int":
            print(repr(shlex.quote(int(sys.argv[2]))))
        elif command == "quote-bytes":
            print(repr(shlex.quote(bytes([int(x) for x in sys.argv[2:]]))))
        else:
            raise ValueError("unknown native request")
    except TypeError as exc:
        print("!TypeError: " + str(exc))
    except ValueError as exc:
        print("!ValueError: " + str(exc))


main()
