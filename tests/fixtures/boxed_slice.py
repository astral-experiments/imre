class Recorder:
    def __getitem__(self, key):
        return key

    def __setitem__(self, key, value):
        self.saved = key, value


class Index:
    def __init__(self, name, value, visits):
        self.name, self.value, self.visits = name, value, visits

    def __index__(self):
        self.visits.append(self.name)
        return self.value


def current():
    path = "/tmp/æ"
    assert (path[:0], path[:1], path[1:]) == ("", "/", "tmp/æ")
    assert path[:] is path
    astral = "🙂🙂"
    assert astral[:1] == "🙂" and astral[:2] is astral

    class Text(str):
        pass

    letter = "æ"
    for result in (letter[::-1], Text(letter)[:], (letter + "\udcff")[::2]):
        assert type(result) is str and result == letter and result is not letter

    holder = [0, 1, 2, 3]
    exec("holder[::2] = [4, 5]", {"holder": holder})
    assert holder == [4, 1, 5, 3]
    exec("del holder[:1], holder[::-2]", {"holder": holder})
    assert holder == [5]
    exec("holder[:] = []", {"holder": holder})
    part = slice(holder)
    print(type(part) is slice, isinstance(part, slice), isinstance(slice, type))
    print(part.start is None, part.stop is holder, part.step is None)
    print(slice in {type(part)}, part.__reduce__()[0] is slice)
    cut = slice(-2, None, -2)
    print(repr(cut), "abcdef"[cut], (1, 2, 3, 4)[cut], list(range(6)[cut]))
    source = bytearray(b"abcd")
    source[slice(1, 3)] = b"X"
    print(bytes(source))
    numbers = [0, 1, 2, 3, 4]
    numbers[::-1] = numbers
    assert numbers == [4, 3, 2, 1, 0]
    try:
        numbers[::2] = []
    except ValueError:
        pass
    else:
        raise AssertionError("extended slice length was not checked")
    del numbers[4:0:-2]
    assert numbers == [4, 3, 1]

    def resize():
        numbers.clear()
        yield 7

    numbers[1:2] = resize()
    assert numbers == [7]
    rec = Recorder()
    key = rec[1:4:2]
    rec[::-1] = key
    print(type(key) is slice, key, rec.saved)
    print(rec[1:3, 4:])
    print(slice(1, 2, 3) == slice(1, 2, 3), slice(1, 2, 4) > slice(1, 2, 3))
    print({slice(1, 2): "found"}[slice(1, 2)])
    slot = slice.__hash__
    assert slot.__objclass__ is slice and cut.__hash__.__self__ is cut
    assert slot(cut) == cut.__hash__() == hash(cut)
    assert slice.__eq__(cut, slice(-2, None, -2)) and cut.__lt__(3) is NotImplemented
    assert slice.__lt__.__get__(slice(1), slice)(slice(2))
    assert slice.__repr__(cut) == cut.__repr__() == repr(cut)
    visits = []
    s = slice(Index("start", -6, visits), Index("stop", 12, visits), Index("step", 2, visits))
    print(s.indices(Index("length", 8, visits)), visits)
    text = "aé中🙂" * 16
    visits.clear()
    assert text[Index("scalar", -1, visits)] == "🙂"
    assert text[Index("start", 1, visits):Index("stop", 4, visits):Index("step", 2, visits)] == "é🙂"
    assert visits == ["scalar", "step", "start", "stop"]
    assert text[len(text):2] == "" and text[-4:-1] == "aé中"
    assert "\ud800🙂z"[::-1] == "z🙂\ud800"
    huge = 10 ** 65
    print(slice(-huge, None, -huge).indices(2 * huge))
    print(list(range(-(2 ** 63), 2 ** 63 - 1)[-3:]))
    try:
        class Derived(slice):
            pass
    except TypeError as error:
        print(str(error))
    for args in [(), (1, 2, 3, 4)]:
        try:
            slice(*args)
        except TypeError as error:
            print(str(error))
    try:
        cut.step = 0
    except AttributeError as error:
        print(str(error))
    try:
        slice(1, 2, 0).indices(4)
    except ValueError as error:
        print(str(error))
    left = right = 0
    for _ in range(4096):
        left = slice(left)
        right = slice(right)
    print(left == right)
