import threading
import warnings
from threading import activeCount, currentThread


thread = threading.Thread(name="original", daemon=False)
with warnings.catch_warnings(record=True) as caught:
    warnings.simplefilter("always")
    assert thread.isDaemon() is False
    assert thread.setDaemon(True) is None and thread.daemon is True
    assert thread.getName() == "original"
    assert thread.setName(42) is None and thread.name == "42"
    assert currentThread() is threading.current_thread()
    assert activeCount() == threading.active_count()
assert len(caught) == 6
assert all(w.category is DeprecationWarning and w.filename == __file__ for w in caught)
assert "activeCount" not in threading.__all__ and "currentThread" not in threading.__all__

with warnings.catch_warnings():
    warnings.simplefilter("error", DeprecationWarning)
    try:
        thread.setName("discarded")
    except DeprecationWarning as error:
        assert "set the name attribute" in str(error)
    else:
        raise AssertionError("the warning filter should have stopped setName")
assert thread.name == "42"
