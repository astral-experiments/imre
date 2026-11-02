import gc
import re


def current():
    pattern = re.compile("(?P<word>x*)|é")
    assert re.sub(pattern, "_", "éx", count=1) == "_éx"
    compile_pattern = re.compile
    try:
        re.compile = None
        assert re.sub(b"a", lambda m: m.group() + b"!", b"aA", flags=re.I) == b"a!A!"
    finally:
        re.compile = compile_pattern
    subject = "éx"
    seen = []

    def replace(match):
        gc.collect()
        assert match.string is subject and match.re is pattern
        seen.append((match.span(), match.group("word")))
        return None if match.group() == "é" else r"\1"

    sub = pattern.sub
    assert sub(repl=replace, string=subject) == r"\1\1\1"
    assert seen == [((0, 0), ""), ((0, 1), None), ((1, 2), "x"), ((2, 2), "")]
    seen.clear()
    assert sub(replace, subject, count=1) == r"\1éx"
    assert sub(replace, subject, count=-1) is subject and len(seen) == 1

    def bad(match):
        seen.append(match.start())
        if match.start() == 2:
            raise ValueError("last callback")
        return 123

    dots = re.compile(".")
    seen.clear()
    try:
        dots.sub(bad, "abc")
    except ValueError as error:
        assert error.args == ("last callback",) and seen == [0, 1, 2]
    else:
        raise AssertionError("callback exception lost")
    for regex, value, item, expected in (
        (re.compile("a"), "ba", 1, "sequence item 1: expected str instance, int found"),
        (re.compile(b"a"), b"ba", "", "sequence item 1: expected a bytes-like object, str found"),
    ):
        try:
            regex.sub(lambda m: item, value)
        except TypeError as error:
            assert str(error) == expected
        else:
            raise AssertionError("invalid replacement accepted")

    source = bytearray(b"za-a-a!")
    result = bytearray(b"x")
    retained = []

    def mutate(match):
        retained.append(match)
        if match.start() == 1:
            try:
                source.extend(b"z")
            except BufferError:
                pass
            else:
                raise AssertionError("match input was resizable")
            source[0] = 90
            source[3] = 98
            return result
        result[0] = 89
        return memoryview(b"y")

    assert re.compile(b"a").sub(mutate, source) == b"zY-b-y!"
    assert [m.start() for m in retained] == [1, 5]
    source.extend(b"?")
    source[1] = 99
    assert retained[0].string is source and retained[0].group() == b"c"

    # The punctuation escape in _strptime.TimeRE.pattern prefixes the captured character.
    assert re.sub(r"([\\.^$*+?\(\){}\[\]|])", r"\\\1", "[é].") == r"\[é\]\."
    groups = re.compile(r"(?P<one>a)?(b)$")
    assert groups.sub(r"<\g<one>/\2/\g<0>>", "b\n") == "</b/b>\n"
    assert re.compile("(|é)").sub(r"[\1]", "é", 2) == "[][é]"
    assert re.compile(b"(.)").sub(bytearray(rb"\377\1"), b"\x81") == b"\xff\x81"
    for count in (-1, 0):
        try:
            groups.sub(r"é\3", "", count)
        except re.PatternError as exc:
            assert (exc.msg, exc.pattern, exc.pos) == ("invalid group reference 3", r"é\3", 2)
        else:
            raise AssertionError("missing group accepted before matching")
    try:
        groups.sub(r"\g<absent>", "b")
    except IndexError as exc:
        assert exc.args == ("unknown group name 'absent'",)
    else:
        raise AssertionError("unknown named group accepted")
    for invalid in ("\\q\\", "\\2\\", "\\400\\", "\\g<x>\\"):
        try:
            groups.sub(invalid, "")
        except re.PatternError as exc:
            assert exc.msg == "bad escape (end of pattern)" and exc.pos == len(invalid) - 1
        else:
            raise AssertionError("incomplete trailing escape ignored")
    try:
        re.compile(b"(x)").sub(memoryview(bytearray(rb"\1")), b"y")
    except ValueError as exc:
        assert str(exc) == "cannot hash writable memoryview object"
    else:
        raise AssertionError("unhashable writable memoryview template accepted")
