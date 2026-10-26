"""Hold a native root and handler for the original incompatible-options test."""
import logging
import sys


def main() -> None:
    root = logging.getLogger("")
    root.setLevel(int(sys.argv[1]))
    assert not root.handlers
    handler: logging.StreamHandler | None = None
    while True:
        op = input()
        if op == "stop":
            for active in root.handlers[:]:
                root.removeHandler(active)
                active.close()
            if handler is not None:
                handler.close()
            return
        try:
            if op == "new":
                handler = logging.StreamHandler()
            elif op == "filename,stream":
                logging.basicConfig(filename=input(), stream=sys.stderr)
            elif op == "filename,handlers":
                assert handler is not None
                logging.basicConfig(filename=input(), handlers=[handler])
            elif op == "stream,handlers":
                assert handler is not None
                logging.basicConfig(stream=sys.stderr, handlers=[handler])
            elif op == "loglevel":
                logging.basicConfig(loglevel=int(input()))
            elif op == "filename=None,filemode":
                logging.basicConfig(filename=None, filemode=input())
            elif op == "filename,encoding,errors,format,level":
                logging.basicConfig(filename=input(), encoding=input(), errors=input(), format=input(), level=int(input()))
            elif op == "filename,encoding,format,level":
                logging.basicConfig(filename=input(), encoding=input(), format=input(), level=int(input()))
            elif op == "debug":
                logging.debug(input())
            elif op == "debug-level":
                print(logging.DEBUG)
                sys.stdout.flush()
                continue
            elif op == "handler-kind":
                print(isinstance(root.handlers[int(input())], logging.FileHandler))
                sys.stdout.flush()
                continue
            elif op == "handler-encoding" or op == "handler-errors":
                found = root.handlers[int(input())]
                if isinstance(found, logging.FileHandler):
                    if op == "handler-encoding":
                        print(found.encoding)
                    else:
                        errors = found.errors
                        print("None" if errors is None else repr(errors))
                else:
                    raise TypeError("root handler is not a FileHandler")
                sys.stdout.flush()
                continue
            elif op == "handler-close":
                root.handlers[int(input())].close()
            elif op == "info":
                print(logging.INFO)
                sys.stdout.flush()
                continue
            elif op == "count":
                print(len(root.handlers))
                sys.stdout.flush()
                continue
            else:
                raise RuntimeError("unexpected native logging command")
        except ValueError as error:
            print("ValueError:" + str(error))
        else:
            print("ok")
        sys.stdout.flush()


main()
