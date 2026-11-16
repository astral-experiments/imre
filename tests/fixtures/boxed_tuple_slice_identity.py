def current():
    pair = (1, 2)
    assert pair[:] is pair and pair[-99:100:1] is pair
    assert pair[1:] == (2,) and pair[::-1] == (2, 1)

    class Pair(tuple):
        pass

    subclass = Pair(pair)
    result = subclass[:]
    assert type(result) is tuple and result == pair and result is not subclass
