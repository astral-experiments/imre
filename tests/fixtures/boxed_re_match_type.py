import typing


def current():
    from typing import Match
    import re

    pattern = re.compile("(a)")
    matched = pattern.match("a")
    byte_match = re.match(b"a", b"a")
    assert type(matched) is type(byte_match) is re.Match
    assert isinstance(matched, re.Match) and isinstance(byte_match, Match)
    assert not isinstance(pattern, Match) and not isinstance(0, re.Match)
    assert type(re.Match) is type and repr(re.Match) == "<class 're.Match'>"
    assert (re.Match.__module__, re.Match.__qualname__) == ("re", "Match")
    assert re.Match.__mro__ == (re.Match, object)
    assert Match is typing.__dict__["Match"] and Match is not re.Match
    assert typing.get_origin(Match) is re.Match and typing.get_args(Match) == ()
    assert Match[str] is Match[str]
    for alias in (re.Match[str], Match[str], re.Match.__class_getitem__(str)):
        assert typing.get_args(alias) == (str,) and typing.get_origin(alias) is re.Match
        try:
            isinstance(matched, alias)
        except TypeError:
            pass
        else:
            raise AssertionError("accepted a parameterized match as classinfo")

    assert re.Match.span.__objclass__ is re.Match
    assert re.Match.span(matched, 1) == (0, 1)
    assert Match.group.__get__(matched, re.Match)(1) == "a"
    assert re.Match.re.__get__(matched) is pattern
    assert re.Match.string.__get__(matched) is matched.string
    for construct in (re.Match, Match, Match[str], lambda: re.Match.__new__(re.Match)):
        try:
            construct()
        except TypeError:
            pass
        else:
            raise AssertionError("constructed a regex match directly")

    try:
        class Sub(re.Match):
            pass
    except TypeError:
        pass
    else:
        raise AssertionError("subclassed re.Match")
