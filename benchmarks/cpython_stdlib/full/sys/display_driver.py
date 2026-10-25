"""One native process retains the builtins namespace and stdout throughout a test."""
import builtins
import sys


def main() -> None:
    saved = sys.stdout
    while True:
        command = input()
        if command == "stop":
            return
        reply = "ok"
        try:
            if command == "display-int":
                sys.__displayhook__(int(input()))
            elif command == "display-none":
                sys.__displayhook__(None)
            elif command == "display-no-args":
                sys.__displayhook__()  # ty: ignore[missing-argument]
            elif command == "get-last":
                reply = "value " + repr(builtins._)
            elif command == "delete-last":
                del builtins._
            elif command == "save-stdout":
                saved = sys.stdout
            elif command == "delete-stdout":
                del sys.stdout
            elif command == "restore-stdout":
                sys.stdout = saved
            else:
                raise ValueError("unknown display operation")
        except AttributeError as error:
            reply = "AttributeError " + str(error)
        except RuntimeError as error:
            reply = "RuntimeError " + str(error)
        except TypeError as error:
            reply = "TypeError " + str(error)
        sys.__stdout__.flush()
        sys.stderr.write(reply + "\n")
        sys.stderr.flush()


main()
