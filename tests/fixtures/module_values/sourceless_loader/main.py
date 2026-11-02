import os
import tempfile
from importlib import abc, machinery, util


with tempfile.TemporaryDirectory() as root:
    path = os.path.join(root, "sample.pyc")
    loader = machinery.SourcelessFileLoader("sample", path)
    assert isinstance(loader, abc.FileLoader)
    assert loader.get_source("sample") is None

    for data, error, message in (
        (b"bad!", ImportError, "bad magic number in 'sample': b'bad!'"),
        (util.MAGIC_NUMBER, EOFError, "reached EOF while reading pyc header of 'sample'"),
        (util.MAGIC_NUMBER + b"\x04" + bytes(11), ImportError, "invalid flags 4 in 'sample'"),
        (util.MAGIC_NUMBER + bytes(12) + b"N", ImportError, f"Non-code object in {path!r}"),
    ):
        with open(path, "wb") as stream:
            stream.write(data)
        try:
            loader.get_code("sample")
        except error as exc:
            assert str(exc) == message
            if isinstance(exc, ImportError):
                assert (exc.name, exc.path) == ("sample", path)
        else:
            raise AssertionError("invalid bytecode loaded")

    spec = util.spec_from_file_location("sample", path)
    assert type(spec.loader) is machinery.SourcelessFileLoader and spec.cached == path
    assert machinery.PathFinder.find_spec("sample", [root]).origin == path
    source = os.path.join(root, "sample.py")
    with open(source, "w"):
        pass
    machinery.PathFinder.invalidate_caches()
    assert machinery.PathFinder.find_spec("sample", [root]).origin == source

    package = os.path.join(root, "sample")
    os.mkdir(package)
    init = os.path.join(package, "__init__.pyc")
    with open(init, "wb"):
        pass
    machinery.PathFinder.invalidate_caches()
    spec = machinery.PathFinder.find_spec("sample", [root])
    assert (spec.origin, spec.submodule_search_locations) == (init, [package])
    assert type(spec.loader) is machinery.SourcelessFileLoader
    with open(os.path.join(package, "__init__.py"), "w"):
        pass
    assert type(machinery.PathFinder.find_spec("sample", [root]).loader) is machinery.SourceFileLoader
