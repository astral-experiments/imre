import re
from array import array


def current():
    pattern = re.compile(r".*?")
    matches = pattern.finditer(string="xαβy", pos=1, endpos=3)
    assert iter(matches) is matches
    first = next(matches)
    second = next(matches)
    assert first is not second and first.re is pattern
    assert first.span() == (1, 1) and second.span() == (1, 2)
    assert [m.span() for m in matches] == [(2, 2), (2, 3), (3, 3)]
    assert next(matches, None) is None
    assert [m.group() for m in re.finditer(pattern=b"a", string=b"Aa", flags=re.I)] == [b"A", b"a"]

    cursor = pattern.scanner(string="xαβy", pos=1, endpos=3)
    scanner_type = type(cursor)
    assert type(scanner_type) is type
    assert (scanner_type.__module__, scanner_type.__qualname__) == ("_sre", "SRE_Scanner")
    assert scanner_type.__new__ is object.__new__
    for construct in (lambda: scanner_type(), lambda: scanner_type.__new__(scanner_type)):
        try:
            construct()
        except TypeError as exc:
            assert str(exc) == "cannot create '_sre.SRE_Scanner' instances"
        else:
            raise AssertionError("constructed a scanner without a compiled pattern")
    try:
        scanner_type.foo = 1
    except TypeError as exc:
        assert str(exc) == "cannot set 'foo' attribute of immutable type '_sre.SRE_Scanner'"
    else:
        raise AssertionError("modified the scanner type")
    print(cursor.pattern is pattern)
    for advance in (cursor.search, cursor.match, cursor.match, cursor.search,
                    cursor.search, cursor.search, cursor.match):
        found = advance()
        print(found and found.span())

    anchored = re.compile("a").scanner("ba")
    print(anchored.match() is None, anchored.search() is None)
    end = re.compile("$").scanner("abc\n")
    print(end.search().span(), end.search().span(), end.search() is None)

    reversed_cursor = re.compile("").scanner("aa", 1, 0)
    try:
        empty_match = reversed_cursor.match()
    except NotImplementedError as error:
        assert str(error) == "zero-width matching with pos past endpos is unsupported"
    else:
        assert empty_match.span() == (1, 1) and reversed_cursor.match() is None
    reversed_cursor = re.compile("").scanner("aa", 1, 0)
    assert reversed_cursor.search() is None and reversed_cursor.match() is None
    assert re.compile("a").scanner("aa", 1, 0).match() is None

    data = bytearray(b"abcd")
    matches = re.finditer(b"(bc)d", data)
    m = next(matches)
    assert m.string is data
    data[1] = ord("B")
    assert m.group(1) == b"Bc"
    try:
        data.extend(b"x")
    except BufferError:
        pass
    else:
        raise AssertionError("a live iterator must keep the buffer exported")
    assert next(matches, None) is None
    data[:] = b"x"
    assert (m.group(), m.group(1), m.span(), m.span(1)) == (b"", b"", (1, 4), (1, 3))

    cursor = re.compile(b"q").scanner(data)
    assert cursor.search() is None
    try:
        data.extend(b"y")
    except BufferError:
        pass
    else:
        raise AssertionError("an exhausted scanner must keep the buffer exported")
    del cursor
    data.extend(b"y")

    view = memoryview(bytearray(b"_id=12!"))[1:]
    pattern = re.compile(rb"id=([0-9]+)")
    scanner = pattern.scanner(view)
    matches = pattern.finditer(view)
    hit = next(matches)
    assert hit.string is view and hit.span(1) == (3, 5)
    view[3:5] = b"34"
    assert hit.group(1) == scanner.search().group(1) == b"34"
    for message in ("memoryview has 2 exported buffers", "memoryview has 1 exported buffer"):
        try:
            view.release()
        except BufferError as error:
            assert str(error) == message
        else:
            raise AssertionError("released a view exported to a live scanner")
        assert next(matches, None) is None
    assert scanner.search() is None
    del scanner
    view.release()
    assert hit.span(1) == (3, 5)

    numbers = array("i", [513, -1])
    raw = numbers.tobytes()
    match = re.compile(b"(?s)(..)").search(numbers, 1, 3)
    assert match.string is numbers and match.span(1) == (1, 3)
    assert match.group(1) == raw[1:3]
    try:
        re.search("x", numbers)
    except TypeError as exc:
        assert str(exc) == "cannot use a string pattern on a bytes-like object"
    else:
        raise AssertionError("text pattern accepted a numeric array")

    matches = re.finditer(b"(?s).", numbers)
    first = next(matches)
    try:
        numbers.append(0)
    except BufferError:
        pass
    else:
        raise AssertionError("a scanner must hold the array export")
    assert [m.group() for m in matches] == [raw[i : i + 1] for i in range(1, len(raw))]
    numbers.append(0)
    assert first.group() == raw[:1]

    calls = []

    def action(scanner, token):
        m = scanner.match
        calls.append((token, m.group(), m.lastindex, m.span(), m.string, m.re is scanner.scanner))
        return int(token)

    # One composite native Pattern captures exactly the lexicon branch selected
    # by priority, so a callback observes the entire subject and absolute span.
    tokens = re.Scanner([(r"[a-zA-Z_]\w*", "name"), (r"\d+", action),
                         (r"=|\+|-|\*|/", "op"), (r"\s+", None)])
    print(tokens.scanner.groups, tokens.scanner.pattern, tokens.scanner.flags)
    print(tokens.scanner.scanner("").pattern is tokens.scanner)
    print(tokens.scan("sum=3 + foo !!"), calls)
    print(re.Scanner([("ab", 1), ("a", 2)]).scan("aba!"))
    # A shorter first rule wins. A later anchored rule sees the original
    # subject, and an empty rule stops without invoking its action.
    assert re.Scanner([("a", 1), ("ab", 2)]).scan("ab!") == ([1], "b!")
    assert re.Scanner([("x", 1), ("^a", 2)]).scan("xa") == ([1], "a")

    def empty_action(scanner, token):
        raise AssertionError("called a zero-width token action")

    assert re.Scanner([("x", 1), ("", empty_action)]).scan("x!") == ([1], "!")
    print(re.Scanner([("", 1), ("a", 2)]).scan("a"))
    print(re.Scanner([(r"\w+", 1)]).scan("aé"))

    print(re.Scanner([(r"(?:ab|c)", 1)]).scan("abc"))
    print(re.Scanner([(r"\(\?i:d\)", 1)]).scan("(?i:d)"))
    print(re.Scanner([(r"[(?i)]", 1)]).scan("(?i)"))

    # Scanner's branch parser and its enclosing pattern have separate flag
    # scopes. Imre may reject an unsupported scope, but must not mis-tokenize it.
    for phrase, flags, text, expected in (
        (r"(?i:é)", 0, "É", ([], "É")),
        (r"(?u:\w)", 0, "é", ([1], "")),
        (r"(?-i:d)", re.I, "D", ([], "D")),
        (r"(?:\N{LATIN SMALL LETTER E WITH ACUTE})(?i:é)", 0, "éÉ", ([], "éÉ")),
        (r"(?i)é", 0, "É", ([], "É")),
    ):
        try:
            result = re.Scanner([(phrase, 1)], flags).scan(text)
        except NotImplementedError as exc:
            assert str(exc).startswith("unsupported scanner phrase: ")
        else:
            assert result == expected, (phrase, result)

    # Translation must preserve token priority or reject the unsupported phrase.
    for phrase, text, expected in (
        (r"(?:x$)", "x\n", ([1], "\n")),
        (r"x$|z", "x\n", ([1], "\n")),
        (r"a++a", "aa", ([2, 2], "")),
    ):
        try:
            result = re.Scanner([(phrase, 1), ("a|x", 2)]).scan(text)
        except NotImplementedError as exc:
            assert str(exc).startswith("unsupported scanner phrase: ")
        else:
            assert result == expected, (phrase, result)
    print(re.Scanner([(r"\$", 1), (r"[$]", 2)]).scan("$$"))
    print(re.Scanner([(r"x$", 1), ("x", 2)], re.M).scan("x\n!"))
    print(repr(re.Scanner([("abc", 1)], re.U).scanner))
