"""Keep the binary stream used by one original CPython fixture in a native process."""
import io


def main() -> None:
    initial = bytes([int(input()) for _ in range(int(input()))])
    stream = io.BytesIO(initial)
    print("ready")
    while True:
        operation = input()
        try:
            if operation == "getvalue":
                print(repr(stream.getvalue()))
            elif operation == "read":
                print(repr(stream.read(int(input()))))
            elif operation == "close":
                stream.close()
                print("None")
            elif operation == "stop":
                return
            else:
                raise AssertionError(operation)
        except ValueError as error:
            print("ValueError " + repr(str(error)))


main()
