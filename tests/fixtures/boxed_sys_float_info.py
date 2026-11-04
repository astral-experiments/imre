import sys


def current():
    info = sys.float_info
    from sys import float_info

    assert info is float_info and info is vars(sys)["float_info"]
    owner = type(info)
    assert isinstance(info, tuple)
    assert (owner.__module__, owner.__name__) == ("sys", "float_info")
    fields = (
        "max", "max_exp", "max_10_exp", "min", "min_exp", "min_10_exp",
        "dig", "mant_dig", "epsilon", "radix", "rounds",
    )
    values = (
        1.7976931348623157e308, 1024, 308, 2.2250738585072014e-308, -1021,
        -307, 15, 53, 2.220446049250313e-16, 2, 1,
    )
    assert owner.__match_args__ == fields
    assert (info.n_fields, info.n_sequence_fields, info.n_unnamed_fields) == (11, 11, 0)
    assert len(info) == len(values) and tuple(info) == values
    for index, name in enumerate(fields):
        field = getattr(info, name)
        assert field == info[index] == values[index]
        assert type(field) is type(values[index])
    assert info[-1] == info.rounds and info[3:6] == values[3:6]
    assert owner(values) == info
    try:
        info.max = 0
    except AttributeError:
        pass
    else:
        raise AssertionError("mutable float_info")
    print("boxed sys.float_info")


if __name__ == "__main__":
    current()
