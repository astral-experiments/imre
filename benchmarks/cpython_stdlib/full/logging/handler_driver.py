"""One native Handler lives across all calls from the original test body."""
import logging
import sys


def main() -> None:
    op = input()
    if op != "new":
        raise ValueError("a Handler must be constructed first")
    handler = logging.Handler()
    print("ok")
    sys.stdout.flush()
    while True:
        op = input()
        if op == "stop":
            return
        if op == "set":
            handler.name = input()
            print("ok")
        elif op == "get":
            name = handler.name
            if name is None:
                print("None")
            else:
                print(repr(name))
        elif op == "emit":
            try:
                handler.emit(None)
            except NotImplementedError as exc:
                print("NotImplementedError:" + str(exc))
            else:
                print("ok")
        else:
            raise ValueError("unknown handler operation")
        sys.stdout.flush()


main()
