import _thread
import warnings

threading = __import__("".join(("thread", "ing")))
with warnings.catch_warnings(record=True) as caught:
    warnings.simplefilter("always")
    assert type(threading.RLock(1)) is _thread.RLock
    assert len(caught) == 1 and caught[0].category is DeprecationWarning
    assert caught[0].filename == __file__

threading._CRLock = None
assert type(threading.RLock()) is threading._PyRLock
