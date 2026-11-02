import logging
import pickle


def current():
    manager = logging.root.manager
    registry = manager.loggerDict
    saved = registry.copy()
    old_disable = manager.disable
    try:
        child = logging.getLogger("jobs.worker")
        pending = registry["jobs"]
        assert isinstance(pending, logging.PlaceHolder)
        assert child in pending.loggerMap and child.parent is logging.root
        parent = logging.getLogger("jobs")
        assert registry["jobs"] is parent and child.parent is parent
        assert parent.getChild("worker") is child
        assert parent.getChildren() == {child}
        assert logging.LoggerAdapter(child).manager is manager

        parent.setLevel("INFO")
        assert child.isEnabledFor(logging.INFO)
        manager.disable = "CRITICAL"
        assert child.isEnabledFor(logging.INFO)  # Already cached.
        logging.disable(logging.CRITICAL)
        assert not child.isEnabledFor(logging.INFO)

        snapshot = registry.copy()
        registry.clear()
        replacement = logging.getLogger("jobs.worker")
        assert replacement is not child and replacement.parent is logging.root
        registry.clear()
        registry.update(snapshot)
        assert logging.getLogger("jobs.worker") is child

        separate = logging.Manager(logging.RootLogger(logging.ERROR))
        assert separate.getLogger("jobs").parent is separate.root
        assert separate.loggerDict["jobs"] is not parent

        for protocol in range(pickle.HIGHEST_PROTOCOL + 1):
            assert pickle.loads(pickle.dumps(child, protocol)) is child
        try:
            pickle.dumps(separate.getLogger("jobs"))
        except pickle.PicklingError as exc:
            assert str(exc) == "logger cannot be pickled"
        else:
            raise AssertionError("pickled a different manager's logger")
    finally:
        registry.clear()
        registry.update(saved)
        logging.disable(old_disable)
