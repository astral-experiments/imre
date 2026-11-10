import warnings


def current():
    checks = []

    class Meta(type):
        def __subclasscheck__(cls, actual):
            checks.append((cls, actual))
            return cls is actual

    class Promoted(UserWarning, metaclass=Meta):
        pass

    message = Promoted("existing")
    with warnings.catch_warnings():
        warnings.simplefilter("error")
        for value in ("text", message):
            checks.clear()
            try:
                warnings.warn_explicit(value, Promoted, "control.py", 1)
            except Promoted as exc:
                assert type(exc) is Promoted
                if value is message:
                    assert exc is message
            else:
                raise AssertionError("warning was not raised")
            assert checks == [(Promoted, Promoted)]

    class Original(Exception, metaclass=Meta):
        pass

    class Changed(Exception):
        pass

    original = Original("original")

    class Cause(Exception):
        def __init__(self):
            original.__class__ = Changed

    checks.clear()
    try:
        raise original from Cause
    except BaseException as exc:
        assert type(exc) is Original and exc is not original
        assert exc.args[0] is original and exc.__cause__ is None
        assert type(original) is Changed and type(original.__cause__) is Cause
    else:
        raise AssertionError("explicit raise did not propagate")
    assert checks == [(Original, Changed)]
