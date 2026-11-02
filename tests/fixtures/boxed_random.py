import random
from random import choice, random as draw


def current():
    assert draw is random.random
    for sample in (draw(), random.random(), draw()):
        assert type(sample) is float and 0.0 <= sample < 1.0
    for call in (lambda: draw(1), lambda: draw(value=1), lambda: draw(1, value=2)):
        try:
            call()
        except TypeError as error:
            print(str(error))

    class Sequence:
        def __init__(self):
            self.lengths = 0

        def __len__(self):
            self.lengths += 1
            return 3 if self.lengths == 1 else 1

        def __getitem__(self, index):
            assert index == 0
            return self

        def __bool__(self):
            raise AssertionError("choice called bool")

    assert choice is random.choice
    seq = Sequence()
    assert choice(seq=seq) is seq and seq.lengths == 2
    try:
        choice([])
    except IndexError as error:
        print(str(error))
    else:
        raise AssertionError("empty choice succeeded")

    for call in (lambda: choice([0], self=1), lambda: choice(sequ=[1])):
        try:
            call()
        except TypeError as error:
            print(str(error))
        else:
            raise AssertionError("invalid choice keyword accepted")

    # A single possible result tests both directions without depending on generator state.
    from random import randrange
    assert randrange is random.randrange
    assert randrange(1, None, 1) == 0
    assert randrange(start=-5, stop=-2, step=4) == -5
    assert randrange(9, 2, -8) == 9
    bound = 1 << 140
    assert 0 <= randrange(bound) < bound
    assert randrange(bound, bound - 1, -bound) == bound

    class Index:
        def __init__(self, value):
            self.value = value

        def __index__(self):
            events.append(("index", self.value))
            return self.value

        def __format__(self, spec):
            assert spec == ""
            events.append(("format", self.value))
            return "bound" + str(self.value)

    events = []
    try:
        randrange(Index(3), Index(6), Index(-2))
    except ValueError as error:
        assert str(error) == "empty range in randrange(bound3, bound6, bound-2)"
    else:
        raise AssertionError("empty descending range accepted")
    assert events == [(kind, value) for kind in ("index", "format") for value in (3, 6, -2)]

    for call in (lambda: randrange(), lambda: randrange(start=1, stopt=3),
                 lambda: randrange(1, start=2), lambda: randrange(1, 2, 3, 4),
                 lambda: randrange(5, None, True), lambda: randrange(1.0)):
        try:
            call()
        except TypeError as error:
            print(str(error))
        else:
            raise AssertionError("invalid randrange argument accepted")
    for call in (lambda: randrange(0), lambda: randrange(7, 7), lambda: randrange(1, 2, 0)):
        try:
            call()
        except ValueError as error:
            print(str(error))
        else:
            raise AssertionError("invalid range accepted")
