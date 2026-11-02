import os
import tempfile
from importlib import machinery
from importlib.abc import PathEntryFinder
from importlib.machinery import FileFinder

events = []


def loader(name, path):
    events.append((name, path))
    return machinery.SourceFileLoader(name, path)


hook = FileFinder.path_hook((loader, [".sample", ".py"]))
assert hook("").path == os.getcwd()
with tempfile.TemporaryDirectory() as root:
    module = os.path.join(root, "item.py")
    package = os.path.join(root, "item")
    namespace = os.path.join(root, "ns")
    os.mkdir(package)
    os.mkdir(namespace)
    init = os.path.join(package, "__init__.sample")
    with open(init, "w") as source:
        source.write("value = 41\n")
    with open(module, "w") as source:
        source.write("value = 42\n")

    finder = hook(root)
    assert isinstance(finder, PathEntryFinder)
    spec = finder.find_spec("some.item")
    assert events == [("some.item", init)]
    assert spec.origin == init and spec.has_location
    assert spec.submodule_search_locations == [package] and spec.loader.path == init
    scope = {}
    exec(spec.loader.get_code("some.item"), scope)
    assert scope["value"] == 41
    os.remove(init)
    spec = finder.find_spec("some.item")
    assert spec.origin == module and spec.submodule_search_locations is None

    spec = finder.find_spec("other.ns")
    assert spec.loader is None and spec.origin is None
    assert spec.submodule_search_locations == [namespace]
    stamp = os.stat(root)
    assert finder.find_spec("late") is None
    with open(os.path.join(root, "late.py"), "w") as source:
        source.write("value = 43\n")
    os.utime(root, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))
    assert finder.find_spec("late") is None
    finder.invalidate_caches()
    assert finder.find_spec("late").loader.name == "late"

    try:
        hook(module)
    except ImportError as exc:
        assert exc.path == module and str(exc) == "only directories are supported"
    else:
        raise AssertionError("a regular file passed the directory path hook")
print("filesystem finder ok")
