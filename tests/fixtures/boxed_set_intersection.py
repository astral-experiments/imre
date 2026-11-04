def current():
    class Frozen(frozenset):
        def __iter__(self):
            raise AssertionError("set storage must not dispatch __iter__")

    class Key:
        calls = 0

        def __hash__(self):
            self.calls += 1
            return 7

        def __eq__(self, other):
            return isinstance(other, Key)

    key = Key()
    other = Frozen([key])
    for kind in (set, frozenset):
        source = kind([key])
        for result in (source.intersection(), kind.intersection(source, source)):
            assert type(result) is kind and result is not source and result == source
        before = key.calls
        assert source.intersection(other) == source and key.calls == before
        cursor = iter([key, "unconsumed"])
        assert source.intersection(cursor, other) == source and key.calls == before + 1
        assert next(cursor) == "unconsumed"
        assert source.intersection([1]) == kind() and len(source) == 1

        try:
            kind().intersection([[]])
        except TypeError as error:
            assert "unhashable" in str(error)
        else:
            raise AssertionError("empty receiver skipped hashing an iterable member")

        try:
            source.intersection(others=other)
        except TypeError:
            pass
        else:
            raise AssertionError("accepted a keyword argument")

    equal_key = Key()
    assert next(iter(other.intersection([equal_key]))) is equal_key
    assert equal_key.calls == 1
    assert type(other.intersection(iter([key]))) is frozenset

    for make_set in (set, frozenset):
        punctuation = make_set("!?\x00\x7f")
        assert punctuation.intersection("clean é雪😀") == make_set()
        assert punctuation.intersection("é?雪!😀?") == make_set("?!")
        assert punctuation.intersection("\x00雪\x7f") == make_set("\x00\x7f")
        assert type(punctuation.intersection("")) is make_set
        assert make_set(["ab", "é"]).intersection("abcé") == make_set(["é"])

    class Chars(str):
        def __iter__(self):
            yield "!"

    assert {"!"}.intersection(Chars("clean")) == {"!"}

    class EqualToX:
        def __hash__(self):
            return hash("x")

        def __eq__(self, other):
            return other == "x"

    assert {EqualToX()}.intersection("x") == {"x"}
