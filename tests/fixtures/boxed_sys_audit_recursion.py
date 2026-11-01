import sys


def current():
    sys.addaudithook(sys.audit)
    try:
        sys.audit("outer")
    except RecursionError:
        print("recursive builtin produced RecursionError")
    else:
        raise AssertionError("expected recursion guard")
