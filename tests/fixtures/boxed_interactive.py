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

    class Unencodable:
        def __repr__(self):
            return chr(0xd800)

    class AsciiStream:
        encoding = "ascii"

        def __init__(self):
            self.buffer = io.BytesIO()

        def write(self, text):
            if text != "\n":
                raise UnicodeEncodeError("ascii", text, 0, 1, "unencodable")
            self.buffer.write(b"\n")

    stream = AsciiStream()
    sys.stdout = stream
    try:
        sys.__displayhook__(Unencodable())
    finally:
        sys.stdout = original
    assert stream.buffer.getvalue() == b"\\ud800\n"

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


def original_stream():
    class Render:
        def __repr__(self):
            return text

    for text in (chr(0xd800), chr(0xdc80)):
        sys.stdout.write("pending")
        sys.__displayhook__(Render())
        sys.stdout.flush()

    text = "A" + chr(0xdc80) + chr(0x100) + chr(0xdc81) + "Z"
    try:
        count = sys.stdout.write(text)
    except UnicodeEncodeError as error:
        assert error.object is text
        print(error.encoding, error.start, error.end)
    else:
        assert count == len(text)


def first_write(hook):
    text = chr(0xd800)
    if hook:
        class Render:
            def __repr__(self):
                return text

        sys.__displayhook__(Render())
    else:
        try:
            sys.stdout.write(text)
        except UnicodeEncodeError:
            sys.stdout.write("A")
        else:
            raise AssertionError("strict encoding accepted a lone surrogate")


if __name__ == "__main__":
    current()
