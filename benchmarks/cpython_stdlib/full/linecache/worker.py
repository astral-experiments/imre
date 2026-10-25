"""Keep native cache state across operations invoked by one real test fixture."""

import linecache
import sys


def serve() -> None:
    while True:
        op = input()
        if op == "quit":
            return
        name = input()
        if op == "lines":
            try:
                print(repr(linecache.getlines(name)))
            except LookupError as exc:
                print("LookupError:" + str(exc))
            except NotImplementedError as exc:
                print("NotImplementedError:" + str(exc))
        elif op == "frozen":
            print(repr(linecache.getlines(name, {"__file__": input()})))
        elif op == "line":
            print(repr(linecache.getline(name, int(input()))))
        elif op == "float":
            number = float(input())
            try:
                print(repr(linecache.getline(name, number)))
            except TypeError as exc:
                print("TypeError:" + str(exc))
        elif op == "update":
            print(repr(linecache.updatecache(name)))
        elif op == "check":
            linecache.checkcache(name)
            print("None")
        elif op == "all":
            linecache.checkcache()
            print("None")
        elif op == "clear":
            linecache.clearcache()
            print("None")
        elif op == "contains":
            print(name in linecache.cache)
        elif op == "lazy":
            print(linecache.lazycache(name, None))
        else:
            raise AssertionError("unknown operation")
        sys.stdout.flush()


serve()
