"""One live native StringIO per child, driven by the original CPython test body."""

from io import StringIO


def text_argument() -> str:
    # Newlines and NUL are test data, so send characters as decimal code points.
    text = ""
    for _ in range(int(input())):
        text += chr(int(input()))
    return text


def run() -> None:
    initial = text_argument()
    if input() == "none":
        stream = StringIO(initial, None)
    else:
        stream = StringIO(initial, text_argument())
    print("ready")
    while True:
        command = input()
        if command == "stop":
            return
        try:
            if command == "write":
                print(repr(stream.write(text_argument())))
            elif command == "getvalue":
                print(repr(stream.getvalue()))
            elif command == "read":
                print(repr(stream.read(int(input()))))
            elif command == "readlines":
                print(repr(stream.readlines()))
            elif command == "seek":
                offset = int(input())
                print(repr(stream.seek(offset, int(input()))))
            elif command == "tell":
                print(repr(stream.tell()))
            elif command == "writable":
                print(repr(stream.writable()))
            elif command == "readable":
                print(repr(stream.readable()))
            elif command == "seekable":
                print(repr(stream.seekable()))
            elif command == "isatty":
                print(repr(stream.isatty()))
            elif command == "closed":
                print(repr(stream.closed))
            elif command == "newlines":
                print(repr(stream.newlines))
            elif command == "iter":
                print(repr(iter(stream) is stream))
            elif command == "next":
                print(repr(next(stream)))
            elif command == "close":
                stream.close()
                print("None")
            else:
                raise RuntimeError("unknown StringIO driver command")
        except StopIteration:
            print("stop-iteration")
        except ValueError as exc:
            print("ValueError " + repr(str(exc)))
        except OSError as exc:
            print("OSError " + repr(str(exc)))


run()
