"""Keep one native logging registry alive throughout an upstream unittest method."""
import logging
import sys


def main() -> None:
    while True:
        op = input()
        if op == "stop":
            return
        if op == "add":
            number = int(input())
            logging.addLevelName(number, input())
            print("None")
        elif op == "number":
            print(repr(logging.getLevelName(int(input()))))
        elif op == "name":
            value = logging.getLevelName(input())
            if isinstance(value, int):
                print(value)
            else:
                print(repr(value))
        else:
            raise ValueError("unknown level operation")
        sys.stdout.flush()


main()
