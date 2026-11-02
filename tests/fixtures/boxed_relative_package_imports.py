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
