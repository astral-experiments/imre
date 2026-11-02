from enum import Enum, IntEnum, IntFlag, StrEnum, global_enum, global_enum_repr, global_str


SCARLET = None


@global_enum
class Color(Enum):
    RED = 1
    SCARLET = 1


@global_enum
class Status(IntEnum):
    OK = 0


@global_enum
class Label(StrEnum):
    YES = "yes"


@global_enum
class Bits(IntFlag):
    READ = 1
    WRITE = 2


def current():
    module = Color.__module__.split('.')[-1]
    assert RED is SCARLET is Color.RED
    assert OK is Status.OK and YES is Label.YES
    assert READ is Bits.READ and WRITE is Bits.WRITE
    assert repr(SCARLET) == module + ".RED" and str(SCARLET) == "RED"
    assert repr(OK) == module + ".OK" and str(OK) == "0" and OK == 0
    assert repr(YES) == module + ".YES" and str(YES) == "yes"
    for value, shown, scalar in (
        (READ, module + ".READ", "1"),
        (READ | WRITE, module + ".READ|" + module + ".WRITE", "3"),
        (READ | 4, module + ".READ|4", "5"),
        (Bits(4), module + ".Bits(4)", "4"),
        (Bits(0), module + ".Bits(0)", "0"),
    ):
        assert repr(value) == shown and repr([value]) == "[" + shown + "]"
        assert str(value) == scalar
    combo = READ | WRITE
    original_name = combo._name_
    try:
        combo._name_ = "²pair|READ"
        assert repr(combo) == "²pair|" + module + ".READ"
        combo._name_ = "⅕pair|READ"
        assert repr(combo) == module + ".⅕pair|" + module + ".READ"
    finally:
        combo._name_ = original_name

    @global_enum
    class Overlap(IntFlag):
        A = 1
        B = 2
        AB = 3
        ALIAS_AB = 3
        BC = 6
        D = 8
    assert Overlap(7).name == "A|B|AB|BC"
    assert repr(Overlap(31)) == "|".join((module + ".A", module + ".B", module + ".D",
                                         module + ".AB", module + ".BC", "16"))
    class Custom(IntEnum):
        TWO = 2
        def __str__(self):
            return "custom"
    assert global_enum(Custom) is Custom and str(Custom.TWO) == "custom"
    assert global_enum(Custom, update_str=True) is Custom and str(Custom.TWO) == "TWO"
    class NamedBits(IntFlag):
        LOW = 1
        HIGH = 2
    assert global_enum(NamedBits, True) is NamedBits
    assert str(NamedBits(3)) == "LOW|HIGH" and str(NamedBits(4)) == "NamedBits(4)"

    # Decorating the base affects enum classes declared before and after the call.
    class Existing(IntEnum):
        FIRST = 1

    original_repr, original_str = IntEnum.__repr__, IntEnum.__str__
    try:
        assert global_enum(IntEnum) is IntEnum
        assert IntEnum.__repr__ is global_enum_repr and IntEnum.__str__ is int.__repr__
        assert repr(Existing.FIRST) == module + ".FIRST" and str(Existing.FIRST) == "1"
        global_enum(IntEnum, update_str=True)

        class Later(IntEnum):
            SECOND = 2

        assert IntEnum.__str__ is global_str
        assert str(Existing.FIRST) == "FIRST" and str(Later.SECOND) == "SECOND"
    finally:
        IntEnum.__repr__ = original_repr
        IntEnum.__str__ = original_str

    @global_enum
    class CustomBits(IntFlag):
        CUSTOM_RED = 1

        @staticmethod
        def _numeric_repr_(value):
            return "0xcustom%d" % value

    assert CustomBits(3).name == "CUSTOM_RED|0xcustom2"
    assert repr(CustomBits(3)) == module + ".CUSTOM_RED|0xcustom2"

    import re
    from enum import global_flag_repr
    assert re.RegexFlag.__repr__ is global_flag_repr
    assert str(re.I) == repr(re.I) == "re.IGNORECASE"
    print("enum aliases, module members, strings and flags")
