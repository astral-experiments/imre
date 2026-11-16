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

    # An argument can release the receiver's last Python owner before the call.
    import gc
    import weakref

    for factory in (set, frozenset):
        owners = [factory([1, 2])]
        ref = weakref.ref(owners[0])

        def release():
            owners.clear()
            gc.collect()
            assert ref() is not None
            return [2]

        assert owners[0].intersection(release()) == factory([2])
        assert factory([1, 2]).intersection(factory([2, 3]).intersection([2])) == factory([2])

    events = []

    class Overridden(frozenset):
        def intersection(self, value):
            events.append("override")
            return value

    def argument():
        events.append("argument")
        return 3

    assert Overridden().intersection(argument()) == 3
    assert events == ["argument", "override"]
    events.clear()

    class Descriptor:
        def __get__(self, obj, owner):
            events.append("lookup")
            return lambda item: item + 1

    class Unrelated:
        intersection = Descriptor()

    assert Unrelated().intersection(argument()) == 4
    assert events == ["lookup", "argument"]
    events.clear()

    class Missing:
        def __getattr__(self, name):
            events.append(name)
            raise AttributeError(name)

    try:
        Missing().intersection(argument())
    except AttributeError:
        pass
    assert events == ["intersection"]

    for kind in (set, frozenset):
        left = kind([1, 2])
        right = kind([2, 3])
        assert left.copy() == left
        assert left.difference(right) == kind([1])
        assert left.union(right) == kind([1, 2, 3])
        assert left.symmetric_difference(right) == kind([1, 3])
        assert left.issubset(left) and left.issuperset(left)
        assert left.isdisjoint([3]) and not left.isdisjoint(right)
