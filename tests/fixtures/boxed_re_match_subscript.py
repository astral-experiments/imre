import re


class Index:
    def __index__(self):
        return 1


class Raises:
    def __index__(self):
        raise ValueError("failed index")


def current():
    for pattern, text in [(r"(?P<word>é)(b)?", "é"), (br"(?P<word>\xff)(b)?", b"\xff")]:
        match = re.compile(pattern).match(text)
        assert match[0] == text
        assert match[1] == match["word"] == match[Index()] == text
        assert match[2] is None
        for key in (-1, 3, 1 << 100, "missing", b"word", (1, 2), slice(0, 1)):
            try:
                match[key]
            except IndexError as error:
                assert str(error) == "no such group"
            else:
                raise AssertionError("invalid match group accepted")
        try:
            match[Raises()]
        except ValueError as error:
            assert str(error) == "failed index"
        else:
            raise AssertionError("index exception was lost")
    print("text and bytes match groups passed")
