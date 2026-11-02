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

    from copy import copy, deepcopy

    for member in (State.ALIAS, Source.END, Label.ÉTÉ, Bits.ONE | Bits.FOUR):
        assert copy(member) is member and deepcopy([member])[0] is member
        assert member.__deepcopy__(memo={}) is member
        assert type(member).__copy__ is Enum.__copy__

    class Custom(Enum):
        MEMBER = 1

        def __copy__(self):
            return self.name

    assert copy(Custom.MEMBER) == "MEMBER"

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

    class Index:
        def __index__(self):
            return -31

    assert hex(Index()) == "-0x1f"

    calls = []
    class Flags(IntFlag):
        ONE = 1

        @classmethod
        def _numeric_repr_(cls, unknown):
            calls.append(unknown)
            if len(calls) == 1:
                raise ValueError("retry this formatter")
            if len(calls) == 2:
                assert cls(3).name == "ONE|inner"
                return "outer"
            return ("inner",)

    assert Flags(2).name is None and not calls
    try:
        Flags(3)
    except ValueError as exc:
        assert exc.args == ("retry this formatter",)
    else:
        raise AssertionError("expected the formatter exception")
    assert 3 not in Flags._value2member_map_
    member = Flags(3)
    assert member.name == "ONE|inner" and Flags(3) is member
    assert calls == [2, 2, 2]
