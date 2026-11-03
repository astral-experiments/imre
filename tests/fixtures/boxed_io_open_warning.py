import builtins
import gc
import importlib
import os
import sys
import warnings


initial = builtins.open.__self__
del sys.modules["_io"]
fresh = importlib.import_module("_io")
sys.modules["_io"] = initial
assert fresh.open.__self__ is fresh and fresh is not initial

# Keep each bound opener, then replace the names through which Python sees its types.
names = ("FileIO", "BufferedReader", "BufferedWriter", "BufferedRandom", "TextIOWrapper")
original_types = tuple(getattr(initial, name) for name in names)
fresh_types = tuple(getattr(fresh, name) for name in names)
for name in names:
    setattr(fresh, name, None)
sys.modules["io"] = None


def check(opener, types):
    raw, reader, writer, random, text = types
    for mode, size, cls in (("rb", 0, raw), ("rb", -1, reader), ("wb", 16, writer),
                            ("r+b", 32, random), ("r", -1, text)):
        with opener(os.devnull, mode, size) as stream:
            assert type(stream) is cls
            if mode == "r":
                assert type(stream.buffer) is reader and type(stream.buffer.raw) is raw
            elif size:
                assert type(stream.raw) is raw
        assert stream.closed
    with warnings.catch_warnings(record=True) as seen:
        warnings.simplefilter("always", ResourceWarning)
        stream = opener(os.devnull, "rb")
        identity = id(stream)
        description = repr(stream)
        del stream
        gc.collect()
        assert len(seen) == 1
        warning = seen[0]
        assert warning.category is ResourceWarning
        assert id(warning.source) == identity and type(warning.source) is reader
        assert str(warning.message) == "unclosed file " + description


check(builtins.open, original_types)
check(fresh.open, fresh_types)
