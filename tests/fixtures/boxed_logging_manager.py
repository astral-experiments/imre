import logging


def current():
    root = logging.getLogger()
    manager = root.manager
    assert manager is logging.Logger.manager and manager.root is root
    registry = manager.loggerDict
    child = logging.getLogger("hierarchy.mid.child")
    for name in ("hierarchy", "hierarchy.mid"):
        placeholder = registry[name]
        assert isinstance(placeholder, logging.PlaceHolder)
        assert list(placeholder.loggerMap) == [child]
    parent = logging.getLogger("hierarchy")
    middle = logging.getLogger("hierarchy.mid")
    assert child.parent is middle and middle.parent is parent and parent.parent is root
    assert parent.getChild("mid") is middle and parent.getChildren() == {middle}
    parent.setLevel("INFO")
    assert child.isEnabledFor(logging.INFO)
    logging.disable(logging.INFO)
    assert not child.isEnabledFor(logging.INFO)
    logging.disable(logging.NOTSET)
    assert child.isEnabledFor(logging.INFO)
    del registry[child.name]
    assert logging.getLogger(child.name) is not child

    class LocalLogger(logging.Logger):
        pass

    private = logging.Manager(root)
    private.setLoggerClass(LocalLogger)
    other = private.getLogger(parent.name)
    assert type(other) is LocalLogger and other is not parent
    assert other.manager is private and other.parent is root and registry[parent.name] is parent
    try:
        private.getLogger(1)
    except TypeError as exc:
        assert str(exc) == "A logger name must be a string"
    else:
        raise AssertionError("accepted an integer logger name")
    print("logger registry, hierarchy, level cache and private manager")
