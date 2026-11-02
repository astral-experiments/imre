import sys


def current():
    flags = sys.flags
    owner = type(flags)
    assert flags is sys.flags
    assert owner.__module__ == "sys" and owner.__name__ == "flags"
    assert isinstance(flags, tuple) and issubclass(owner, tuple)
    assert (len(flags), flags.n_sequence_fields, flags.n_fields) == (18, 18, 21)
    assert tuple(flags) == tuple(getattr(flags, key) for key in owner.__match_args__)
    assert flags.gil == 1
    assert type(flags.thread_inherit_context) is type(flags.context_aware_warnings) is int
    assert flags.context_aware_warnings in (0, 1)
    assert "context_aware_warnings" not in repr(flags)
    try:
        flags.context_aware_warnings = 2
    except AttributeError as error:
        assert str(error) == "readonly attribute"
    else:
        raise AssertionError("mutable sys.flags")
    for construct, args in ((owner, ()), (owner.__new__, (owner,))):
        try:
            construct(*args)
        except TypeError as error:
            assert str(error) == "cannot create 'sys.flags' instances"
        else:
            raise AssertionError("constructed a second sys.flags")
    print("immutable flags")
