from importlib import _bootstrap_external
from types import SimpleNamespace
import warnings


def current():
    bless = _bootstrap_external._bless_my_loader
    loader = object()
    assert bless(None) is None
    assert bless({}) is None
    assert bless({"__spec__": SimpleNamespace(loader=loader)}) is loader

    for spec, error in [(None, ValueError), (object(), AttributeError), (SimpleNamespace(loader=None), ValueError)]:
        try:
            bless({"__spec__": spec})
        except error as exc:
            assert str(exc) == "Module globals is missing a __spec__.loader"
        else:
            raise AssertionError("missing loader accepted")

    with warnings.catch_warnings(record=True) as seen:
        warnings.simplefilter("always")
        assert bless({"__loader__": loader}) is loader
        assert bless({"__loader__": loader, "__spec__": SimpleNamespace(loader=object())}) is loader
        assert [warning.category for warning in seen] == [DeprecationWarning, DeprecationWarning]
        assert [str(warning.message) for warning in seen] == [
            "Module globals is missing a __spec__.loader",
            "Module globals; __loader__ != __spec__.loader",
        ]

    with warnings.catch_warnings():
        warnings.simplefilter("error", DeprecationWarning)
        try:
            bless({"__loader__": loader})
        except DeprecationWarning:
            pass
        else:
            raise AssertionError("warning filter ignored")
