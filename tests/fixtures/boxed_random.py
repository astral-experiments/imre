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
