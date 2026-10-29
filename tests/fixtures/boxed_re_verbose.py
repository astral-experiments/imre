import re


def current():
    tokens = re.compile(
        r"""
        ([\w#-]*)'  # encoding, possibly empty
        [\w#-]*'    # language
        ([\w#%']+)  # encoded payload
        """,
        re.ASCII | re.VERBOSE,
    )
    print(re.X is re.VERBOSE, tokens.flags, tokens.groups)
    for raw in ("utf-8'en'caf%C3%A9", "''plain", "#'fr'#x", "utfé'en'x", "utf-8'en'"):
        match = tokens.match(raw)
        print(None if match is None else match.groups())
    pattern = re.compile(r"(?P<optional>\w+)? \# ([ #]+) $  # tail", re.X)
    fallback = []
    result = pattern.match("# #\n")
    print(pattern.groups, pattern.flags, result.group(), result.groups())
    print(result.groups(default=fallback)[0] is fallback, result.groups("absent"))
    print(pattern.fullmatch("# #\n") is None, pattern.fullmatch("# #").groups())
    print(pattern.match("é# #").group("optional"))
    print(re.match(r"a \s # ignored", "a\x1c", re.X).groups())
    octets = re.compile(rb"[\x00-\x19\",;\\\x7f-\xff]", re.A)
    print(type(octets.pattern) is bytes, octets.flags, octets.groups)
    print([n for n in range(256) if octets.fullmatch(bytes([n])) is not None])
    raw = bytes([255])
    captured = re.compile(b"(" + raw + b")?(x)").fullmatch(bytearray(raw + b"x"))
    print(captured.group() == raw + b"x", captured.groups() == (raw, b"x"))
    print(re.match(rb"(y)?(x)", b"x").groups(default=fallback)[0] is fallback)
    print(re.compile(rb"(\xff)").findall(raw + raw) == [raw, raw])
    search = re.compile(rb"(\xff)(x)?")
    print(search.search(b"a" + raw + b"x", pos=1, endpos=2).groups())
    print(re.search(r" (é) \# ", "前é#", re.X).groups())
    print(re.compile(rb"(a+) | (b)", re.X).fullmatch(b"b").groups())
    print(re.compile(r"a{,2}").match("aaa").group())
    print(re.compile(rb"(ab){,2}?x").fullmatch(b"ababx").groups())
    print(re.compile(r"[a{,2}]{,}").fullmatch("{,2}").group())
    print([c for c in "0{,2}]" if re.fullmatch(r"[]{,2}]", c)])
    print([c for c in "\b]x" if re.fullmatch(r"[]\b]", c)])
    for pattern, subject in ((octets, "x"), (tokens, b"x")):
        try:
            pattern.match(subject)
        except TypeError as error:
            print(str(error))
