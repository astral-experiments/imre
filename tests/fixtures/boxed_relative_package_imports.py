def current():
    from importlib import machinery
    namespace = {"__spec__": machinery.ModuleSpec("importlib.example", None)}
    exec("from .machinery import ModuleSpec", namespace)
    assert namespace["ModuleSpec"] is machinery.ModuleSpec
    parent = {"__package__": "importlib.nested"}
    assert __import__("machinery", parent, fromlist=("ModuleSpec",), level=2) is machinery
    assert __import__("", parent, fromlist=("machinery",), level=2).machinery is machinery
    try:
        __import__("sys", parent, level=3)
    except ImportError as error:
        assert str(error) == "attempted relative import beyond top-level package"
    else:
        raise AssertionError("relative import escaped its package")

    import importlib
    import sys
    from importlib import _bootstrap

    class Registry(dict):
        @property
        def get(self):
            _bootstrap._NEEDS_LOADING = object()

            def lookup(key, *default):
                if default:
                    assert default[0] is _bootstrap._NEEDS_LOADING
                if isinstance(sys.exception(), ModuleNotFoundError):
                    assert key is finder.fullname and sys.exception().name is finder
                return original.get(key, *default)

            return lookup

    class Finder:
        def find_spec(self, fullname, path, target=None):
            if fullname == "importlib.missing_child_for_fromlist":
                self.fullname = fullname
                raise ModuleNotFoundError(name=self)

        def __eq__(self, other):
            assert other is self.fullname
            sys.modules = Registry(original)
            return True

    original = sys.modules
    sentinel = _bootstrap._NEEDS_LOADING
    finder = Finder()
    sys.meta_path.insert(0, finder)
    try:
        assert __import__("importlib", fromlist=("missing_child_for_fromlist",)) is importlib
        name = ".".join(("importlib", "missing_child_for_fromlist"))
        try:
            importlib.import_module(name)
        except ModuleNotFoundError as exc:
            assert exc.name is finder and finder.fullname is name
        else:
            raise AssertionError("finder exception was lost")
    finally:
        sys.modules = original
        _bootstrap._NEEDS_LOADING = sentinel
        sys.meta_path.remove(finder)
