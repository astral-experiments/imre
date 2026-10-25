"""Template: factories at the upstream code sites precede this worker."""
import sys
import typing
from typing import Callable, clear_overloads, get_overloads, overload


def serve() -> None:
    functions: list[Callable[[], None]] = []
    while True:
        operation = input()
        if operation == "quit":
            return
        if operation == "new":
            target = make(int(input()))
            target.__module__ = input()
            target.__qualname__ = input()
            target.__name__ = input()
            if input() == "1":
                target.__deprecated__ = input()
            functions.append(target)
            print(len(functions) - 1)
        elif operation == "register":
            stub = overload(functions[int(input())])
            print(stub is typing._overload_dummy)
        elif operation == "get":
            result = get_overloads(functions[int(input())])
            handles: list[int] = []
            for original in result:
                for index in range(len(functions)):
                    if original is functions[index]:
                        handles.append(index)
                        break
                else:
                    raise AssertionError("registry returned an unknown native function")
            print(repr(handles))
        elif operation == "marker":
            slot = int(input())
            try:
                print(repr(functions[slot].__deprecated__))
            except AttributeError:
                print("None")
        elif operation == "register-builtin":
            if input() == "sum":
                returned = overload(sum)
            else:
                returned = overload(print)
            print(returned is typing._overload_dummy)
        elif operation == "get-builtin":
            if input() == "sum":
                print(repr(get_overloads(sum)))
            else:
                print(repr(get_overloads(print)))
        elif operation == "stub":
            try:
                typing._overload_dummy()
            except NotImplementedError as error:
                print(repr(str(error)))
        elif operation == "size":
            print(len(typing._overload_registry))
        elif operation == "clear":
            clear_overloads()
            print("None")
        else:
            raise AssertionError("unknown registry request")
        sys.stdout.flush()


serve()
