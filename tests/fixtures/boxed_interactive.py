import io
import sys


def current():
    namespace = {}
    output = io.StringIO()
    original = sys.stdout
    original_hook = sys.displayhook
    sys.displayhook = sys.__displayhook__
    sys.stdout = output
    try:
        for prompt in [
            "from math import floor",
            "x = floor(3.5); x",
            "for i in range(2):\n    (i, x)\n",
            "def call():\n    'docstring'\n    x + 1\n    return 'answer'\n",
            "call()",
            "_",
            "2 + 1j",
            "print('done'); None",
        ]:
            exec(compile(prompt, "<doctest>", "single"), namespace)
    finally:
        sys.stdout = original
        sys.displayhook = original_hook
    assert output.getvalue() == "3\n(0, 3)\n(1, 3)\n'answer'\n'answer'\n(2+1j)\ndone\n"

    events = []
    sys.displayhook = events.append
    try:
        for prompt in [
            "class Box:\n    11\n    def value(self):\n        12\n        return 13\n",
            "Box().value()",
            "def outer():\n    def inner():\n        21\n        return 22\n    inner()\n    return 23\n",
            "outer()",
            "1; None; 2",
        ]:
            exec(compile(prompt, "<doctest>", "single"), namespace)
        exec(compile("31", "<script>", "exec"), namespace)
    finally:
        sys.displayhook = original_hook
    assert events == [13, 23, 1, None, 2]
    try:
        compile("1\n2\n", "<doctest>", "single")
    except SyntaxError:
        print("interactive stdout, globals, and hook verified")
    else:
        raise AssertionError("expected a single interactive input")


if __name__ == "__main__":
    current()
