import importlib
import sys

first = importlib.import_module(sys.argv[1])
if first.__name__ == "_pickle":
    assert "pickle" not in sys.modules
accelerator = importlib.import_module("_pickle")
pickle = importlib.import_module("pickle")

assert accelerator is not pickle
writer = accelerator.dumps
assert writer is pickle.dumps and writer.__self__ is accelerator
assert writer(42, protocol=4) == b"\x80\x04K*."
for error in (accelerator.PickleError, accelerator.PicklingError, accelerator.UnpicklingError):
    assert getattr(pickle, error.__name__) is error
    assert pickle.loads(writer(error, 4)) is error


def check_held_writer():
    def local():
        pass

    error = accelerator.PicklingError
    del accelerator.dumps
    del accelerator.PicklingError
    assert not hasattr(accelerator, "dumps") and pickle.dumps is writer
    try:
        writer(local)
    except error:
        pass
    else:
        raise AssertionError("serialized a local function")


check_held_writer()
