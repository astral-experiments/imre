import contextlib
import os
from contextlib import chdir

assert chdir is contextlib.chdir
original = os.getcwd()
target = os.path.join(original, "target")
os.mkdir(target)
try:
    manager = chdir(path=target)
    try:
        with manager as entered:
            assert entered is None
            with chdir(original):
                with manager:
                    assert os.getcwd() == target
                assert os.getcwd() == original
            assert os.getcwd() == target
            raise RuntimeError("body")
    except RuntimeError as error:
        assert str(error) == "body"
    else:
        raise AssertionError("chdir suppressed the body exception")
    assert os.getcwd() == original

    try:
        with chdir(os.path.join(target, "missing")):
            raise AssertionError("entered a missing directory")
    except FileNotFoundError:
        pass
    assert os.getcwd() == original
finally:
    os.rmdir(target)
