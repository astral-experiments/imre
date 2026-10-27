from enum import Enum, IntEnum, IntFlag, StrEnum, auto


def current():
    marker = auto()

    class State(Enum):
        FIRST = ALSO_FIRST = marker
        LAST = FIRST + 7
        ALIAS = LAST
        END = auto()

    print(tuple(State.__members__), [item.value for item in State])
    print(State.END is State(9), State.ALIAS is State["LAST"], marker.value)

    class Source(IntEnum):
        START = auto()
        END = auto()

    class Label(StrEnum):
        ÉTÉ = auto()

    class Bits(IntFlag):
        ONE = auto()
        TWO = auto()
        FOUR = auto()

    print({Source.END: "end"}[2], Source.START < Source.END, Label.ÉTÉ.value, Bits.FOUR.value)

    history = []

    class Generator(Enum):
        @staticmethod
        def _generate_next_value_(name, start, count, previous):
            history.append((name, start, count, tuple(previous)))
            previous.append("local")
            return count + 1

    class Tagged(Generator):
        ONE = auto()
        ALIAS = ONE
        TWO = auto(), auto()

        @classmethod
        def first(cls):
            return cls.ONE

    print(Tagged.first() is Tagged.ONE, Tagged.TWO.value, history)

    try:
        class Mixed(Enum):
            TEXT = "red"
            NUMBER = 2
            NEXT = auto()
    except TypeError as error:
        print(str(error))

    try:
        class Late(Enum):
            FIRST = auto()
            def _generate_next_value_(name, start, count, last_values):
                return name
    except TypeError as error:
        print(str(error))
