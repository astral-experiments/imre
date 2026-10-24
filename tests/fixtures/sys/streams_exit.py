import sys
from sys import stderr, stdout

def stop(which: str) -> None:
    try:
        if which == "none":
            sys.exit()
        if which == "explicit":
            sys.exit(None)
        if which == "number":
            sys.exit(47)
        if which == "huge":
            sys.exit(1 << 90)
        if which == "text":
            sys.exit("23")
        if which == "empty":
            sys.exit("")
        if which == "bool":
            sys.exit(True)
        if which == "raised":
            raise SystemExit(11)
        if which == "class":
            raise SystemExit
        if which == "unicode":
            sys.exit("é🦀")
        sys.exit(-3)
    finally:
        stderr.write("cleanup,")

def main() -> None:
    if sys.argv[1] == "stdio":
        print(sys.platform)
        print(stdout.write("hé🦀"))
        stderr.write("part")
        stdout.flush()
        stderr.write("-end\n")
        stderr.flush()
        return
    if sys.argv[1] == "catch":
        for which in ["none", "explicit", "number", "huge", "text", "empty", "bool", "raised", "class", "unicode"]:
            try:
                stop(which)
            except Exception:
                print("caught exception unexpectedly")
            except SystemExit as exc:
                print(exc.code)
                print(str(exc))
                print(exc.code == 47)
                print("23" == exc.code)
                print(exc.code is None)
                print(bool(exc.code))
                print(isinstance(exc.code, int))
                print(isinstance(exc.code, (bool, str)))
                if isinstance(exc, Exception):
                    print("not an ordinary Exception")
        return
    stop(sys.argv[1])

main()
