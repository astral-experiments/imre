import random
from random import choice, choices, randbytes, randint, random as draw, shuffle


def current():
    seed = (1 << 100) + 7
    random.seed(seed)
    first = random.Random(seed)
    second = random.Random(-seed)
    for source in (random, first, second):
        assert source.getrandbits(0) == 0
        try:
            source.getrandbits(-1)
        except ValueError as error:
            print(str(error))
        else:
            raise AssertionError("negative bit width accepted")
    for width in (1, 32, 33, 64, 129):
        value = random.getrandbits(width)
        assert first.getrandbits(width) == second.getrandbits(width) == value
        print(value)
    print(random.random(), first.random(), second.random())

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

    assert choices is random.choices
    assert choices([seq])[0] is seq
    assert choices([None, seq], (w for w in [0, 1]), k=3) == [seq, seq, seq]
    assert random.choices(population=[None, seq], cum_weights=[0, 2])[0] is seq
    assert choices([], k=0) == []

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
    assert randint is random.randint
    assert randint(b=Index(bound), a=Index(bound)) == bound
    assert events == [("index", bound), ("index", bound)]
    try:
        randint(Index(2), Index(1))
    except ValueError as error:
        assert str(error) == "empty range in randint(2, 1)"
    else:
        raise AssertionError("empty integer interval accepted")

    assert randbytes is random.randbytes
    for size in (0, 1, 4, 5, 35):
        value = randbytes(n=size)
        assert type(value) is bytes and len(value) == size

    class Length:
        def __mul__(self, other):
            assert other == 8
            return 0

        def __index__(self):
            return 3

    assert randbytes(Length()) == bytes(3)
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

    assert shuffle is random.shuffle
    assert shuffle(x=[]) is None
    assert shuffle((0,)) is None
    try:
        shuffle((0, 1))
    except TypeError:
        pass
    else:
        raise AssertionError("immutable pair was shuffled")

    class Marker(Exception):
        pass

    class Token:
        def __init__(self, number):
            self.number = number

    class SequenceProbe:
        def __len__(self):
            events.append("len")
            return 3

        def __getitem__(self, index):
            import gc
            gc.collect()
            events.append(("get", index))
            return Token(len(events))

        def __setitem__(self, index, value):
            import gc
            gc.collect()
            events.append(("set", index, value.number))
            if len(events) == 4:
                self.first_write = value
            else:
                raise Marker(value)

    events = []
    probe = SequenceProbe()
    try:
        shuffle(x=probe)
    except Marker as error:
        assert error.args[0].number == 3
    else:
        raise AssertionError("setter exception was lost")
    assert probe.first_write.number == 2
    j = events[1][1]
    assert 0 <= j <= 2
    assert events == ["len", ("get", j), ("get", 2), ("set", 2, 2), ("set", j, 3)]

    from math import log
    from random import expovariate

    assert expovariate is random.expovariate
    random.seed(seed)
    reference = random.Random(seed)
    assert expovariate() == -log(1.0 - reference.random())
    assert random.random() == reference.random()
    assert random.Random(seed).expovariate(lambd=-2) == -log(1.0 - random.Random(seed).random()) / -2

    class Fixed(random.Random):
        def random(self):
            draws.append(1)
            return 0.25

    class Duck:
        random = Fixed.random

    draws = []
    source = Fixed()
    assert source.expovariate() == -log(0.75)
    assert random.Random.expovariate(self=Duck(), lambd=2) == -log(0.75) / 2
    assert len(draws) == 2
    for call, error, count in (
        (lambda: source.expovariate(1, lambd=2), TypeError, 2),
        (lambda: source.expovariate(0), ZeroDivisionError, 3),
        (lambda: source.expovariate(None), TypeError, 4),
    ):
        try:
            call()
        except (TypeError, ZeroDivisionError) as caught:
            assert type(caught) is error and len(draws) == count
        else:
            raise AssertionError("invalid exponential rate accepted")
