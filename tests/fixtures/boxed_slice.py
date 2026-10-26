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
    holder = []
    part = slice(holder)
    print(type(part) is slice, isinstance(part, slice), isinstance(slice, type))
    print(part.start is None, part.stop is holder, part.step is None)
    print(slice in {type(part)}, part.__reduce__()[0] is slice)
    cut = slice(-2, None, -2)
    print(repr(cut), "abcdef"[cut], (1, 2, 3, 4)[cut], list(range(6)[cut]))
    source = bytearray(b"abcd")
    source[slice(1, 3)] = b"X"
    print(bytes(source))
    rec = Recorder()
    key = rec[1:4:2]
    rec[::-1] = key
    print(type(key) is slice, key, rec.saved)
    print(rec[1:3, 4:])
    print(slice(1, 2, 3) == slice(1, 2, 3), slice(1, 2, 4) > slice(1, 2, 3))
    print({slice(1, 2): "found"}[slice(1, 2)])
    visits = []
    s = slice(Index("start", -6, visits), Index("stop", 12, visits), Index("step", 2, visits))
    print(s.indices(Index("length", 8, visits)), visits)
    huge = 10 ** 65
    print(slice(-huge, None, -huge).indices(2 * huge))
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
