import random
from random import random as draw


def current():
    assert draw is random.random
    for sample in (draw(), random.random(), draw()):
        assert type(sample) is float and 0.0 <= sample < 1.0
    for call in (lambda: draw(1), lambda: draw(value=1), lambda: draw(1, value=2)):
        try:
            call()
        except TypeError as error:
            print(str(error))
