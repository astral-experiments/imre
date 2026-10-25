"""Keep native strings alive while the original parent observes their identity."""
import sys


def serve() -> None:
    strings: list[str] = []
    while True:
        command = input()
        if command == "stop":
            return
        try:
            if command == "string":
                strings.append(input())
                print(len(strings) - 1)
            elif command == "intern":
                value = sys.intern(strings[int(input())])
                found = -1
                for index in range(len(strings)):
                    if strings[index] is value:
                        found = index
                        break
                if found == -1:
                    strings.append(value)
                    found = len(strings) - 1
                print(found)
            elif command == "query":
                print(sys._is_interned(strings[int(input())]))
            elif command == "intern-empty":
                sys.intern()
            elif command == "query-empty":
                sys._is_interned()
            elif command == "intern-bytes":
                sys.intern(input().encode())
            elif command == "query-bytes":
                sys._is_interned(input().encode())
            else:
                raise ValueError("unknown command")
        except TypeError as error:
            print("TypeError: " + str(error))
        sys.stdout.flush()


serve()
