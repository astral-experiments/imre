"""Keep one native interpreter's limit alive for an unchanged test method."""
import sys
from sys import getrecursionlimit, setrecursionlimit

while True:
    op = input()
    if op == "quit":
        break
    try:
        if op == "get":
            print(getrecursionlimit())
        elif op == "get-extra":
            print(getrecursionlimit(42))
        elif op == "set-int":
            setrecursionlimit(int(input()))
            print("None")
        elif op == "set-missing":
            setrecursionlimit()
            print("None")
        else:
            raise AssertionError("unknown recursion operation")
    except TypeError as exc:
        print("TypeError: " + str(exc))
    except ValueError as exc:
        print("ValueError: " + str(exc))
    except RecursionError as exc:
        print("RecursionError: " + str(exc))
    sys.stdout.flush()
