def current():
    class Number(int):
        def __repr__(self):
            return "overridden"

    class Float(float):
        def __repr__(self):
            return "overridden"

    number = Number(-17)
    fraction = Float(-0.0)
    assert repr(number) == repr(fraction) == "overridden"
    assert int.__repr__(number) == "-17"
    assert float.__repr__(fraction) == "-0.0"
    assert int.__repr__(False) == "0" and int.__repr__(True) == "1"
    assert bool.__repr__(False) == "False" and True.__repr__() == "True"
    assert (1e-5).__repr__() == "1e-05"
    bound = int.__repr__.__get__(number, Number)
    assert bound.__self__ is number and bound() == "-17"
    try:
        bool.__repr__(1)
    except TypeError as error:
        print(error)
    else:
        raise AssertionError("bool.__repr__ accepted an int")
