import json
from enum import Enum


def current():
    class Ratio(float, Enum):
        FIRST = "1.25"
        ALIAS = 1.25

    assert Ratio(1.25) is Ratio.FIRST is Ratio.ALIAS
    assert type(Ratio.FIRST.value) is float
    assert repr(Ratio.FIRST) == "<Ratio.FIRST: 1.25>"
    assert json.dumps([Ratio.FIRST]) == "[1.25]"

    initialized = []

    class Tagged(float, Enum):
        def __new__(cls, number, tag):
            obj = float.__new__(cls, number)
            obj._value_ = tag
            return obj

        def __init__(self, number, tag):
            initialized.append((number, self.name))

        FIRST = -0.0, ["left"]
        ALIAS = 5.0, ["left"]
        OTHER = -0.0, ["right"]

    assert initialized == [(-0.0, "FIRST"), (5.0, "ALIAS"), (-0.0, "OTHER")]
    assert Tagged(["left"]) is Tagged.ALIAS is Tagged.FIRST
    assert Tagged(["right"]) is Tagged.OTHER
    assert Tagged.OTHER is not Tagged.FIRST
    assert [member.name for member in Tagged] == ["FIRST", "OTHER"]
    assert json.dumps(Tagged.ALIAS) == "-0.0"
    try:
        repr(Tagged.FIRST)
    except TypeError as error:
        print(str(error))
    else:
        raise AssertionError("float.__repr__ accepted a list value")

    class Edge(float, Enum):
        name = float("nan")
        value = 1

    nan = Edge.name
    assert (nan.name, Edge.value.name) == ("name", "value")
    assert nan != nan and nan in [nan]
    assert json.dumps(nan) == "NaN"
