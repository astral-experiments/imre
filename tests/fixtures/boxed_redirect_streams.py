from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
import sys


def current():
    stdout, stderr = sys.stdout, sys.stderr
    output, errors = StringIO(), StringIO()
    redirect = redirect_stdout(output)
    with redirect as target:
        print("outer", target is output)
        try:
            with redirect_stderr(errors):
                with redirect_stdout(sys.stderr):
                    print("error", end="!")
                    with redirect:
                        print("reentrant", end=".")
                    print("restored", end="?")
                    raise ValueError("stop")
        except ValueError as error:
            print(str(error), file=sys.stdout)
    print(repr(output.getvalue()), repr(errors.getvalue()))
    print(sys.stdout is stdout, sys.stderr is stderr)


if __name__ == "__main__":
    current()
