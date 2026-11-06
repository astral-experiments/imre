from importlib.machinery import SourceFileLoader
import urllib.request


class SplitPath:
    def __str__(self):
        return __file__

    def __fspath__(self):
        raise AssertionError("source reads use the text spelling")


external = SourceFileLoader("plugin", SplitPath())
with open(__file__, "rb") as reference:
    assert external.get_data(external.path) == reference.read()

loader = urllib.request.__loader__
assert isinstance(loader, SourceFileLoader)
source = loader.get_source("urllib.request")
assert "def urlopen(" in source
assert loader.get_data(urllib.request.__file__).decode("utf-8").splitlines() == source.splitlines()

# A separate loader reads the physical file even when its fields match the registered loader.
origin = loader.path
external = SourceFileLoader(loader.name, origin)
try:
    with open(origin, "rb") as physical:
        expected = physical.read()
except FileNotFoundError:
    try:
        external.get_data(origin)
    except FileNotFoundError:
        pass
    else:
        raise AssertionError("external loader returned another loader's source")
else:
    assert external.get_data(origin) == expected
