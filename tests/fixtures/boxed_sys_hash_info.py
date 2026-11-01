import sys


def current():
    info = sys.hash_info
    owner = type(info)
    print(info)
    print(info.width, info.modulus, info.inf, info.nan, info.imag,
          info.algorithm, info.hash_bits, info.seed_bits, info.cutoff)
    assert info is sys.hash_info
    assert info.__class__ is owner and isinstance(info, owner)
    assert isinstance(info, tuple) and issubclass(owner, tuple)
    assert (owner.__module__, owner.__qualname__) == ("sys", "hash_info")
    assert (info.n_fields, info.n_sequence_fields, info.n_unnamed_fields) == (9, 9, 0)
    assert tuple(info) == tuple(getattr(info, key) for key in owner.__match_args__)
    assert info[-1] == info.cutoff and info[:2] == (info.width, info.modulus)
    assert hash(info) == hash(tuple(info))
    assert owner(tuple(info)) == info
    assert hash(info.modulus) == 0 and hash(2 * info.modulus + 7) == 7
    assert hash(float("inf")) == info.inf and hash(float("-inf")) == -info.inf
    assert hash(1j) == info.imag and hash(3 - 2j) == 3 - 2 * info.imag
    try:
        info.width = 1
    except AttributeError:
        pass
    else:
        raise AssertionError("mutable hash_info")
    try:
        owner(())
    except TypeError:
        pass
    else:
        raise AssertionError("accepted a tuple with missing fields")
