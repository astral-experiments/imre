import re
import typing


def current():
    assert "Pattern" not in vars(typing) and "Match" not in vars(typing)
    from typing import Pattern
    assert Pattern is typing.Pattern is vars(typing)["Pattern"]
    assert "Match" not in vars(typing)
    class Replacement:
        pass

    class Name(str):
        pass

    match_type = re.Match
    re.Match = Replacement
    try:
        key = Name("Match")
        replaced = getattr(typing, key)
        assert vars(typing).pop(key) is replaced
        assert replaced.__origin__ is Replacement
        assert type(replaced._name) is str and replaced._name == "Replacement"
        cached = typing.Match
        assert cached.__origin__ is Replacement and typing.Match is cached
        del typing.Match
    finally:
        re.Match = match_type

    from typing import Match
    assert Match is typing.Match is vars(typing)["Match"]

    compiled = re.compile(b"x")
    found = compiled.search(b"yx")
    for alias, origin, value in ((Pattern, re.Pattern, compiled), (Match, re.Match, found)):
        assert typing.get_origin(alias) is origin
        typed = alias[bytes]
        assert typed is alias[bytes] and typing.get_origin(typed) is origin
        assert typing.get_args(typed) == (bytes,)
        assert repr(typed) == "typing." + origin.__name__ + "[bytes]"
        assert alias.__doc__ == "Deprecated alias to re." + origin.__name__ + "."
        assert isinstance(value, alias) and issubclass(type(value), alias)
        try:
            isinstance(value, typed)
        except TypeError as error:
            assert str(error) == "Subscripted generics cannot be used with class and instance checks"
        else:
            raise AssertionError("accepted a parameterized alias for isinstance")
        try:
            alias()
        except TypeError as error:
            assert str(error) == "cannot create 're." + origin.__name__ + "' instances"
        else:
            raise AssertionError("constructed a regex object without a pattern")
    assert not isinstance(compiled, Match) and not isinstance(found, Pattern)
