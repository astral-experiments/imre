"""Thread enrollment lasts as long as the Thread object, independently of start/join."""
import _thread
import gc
import threading
from threading import _dangling
from weakref import WeakSet, ref


def check():
    main = threading.main_thread()
    assert isinstance(main, threading.Thread)
    assert threading.current_thread() is main
    assert isinstance(_dangling, WeakSet) and set(_dangling) == {main}

    first = threading.Thread(name="duplicate")
    second = threading.Thread(name="duplicate")
    assert set(_dangling) == {main, first, second}
    first_ref = ref(first)
    first.cycle = first
    del first
    gc.collect()
    assert first_ref() is None and set(_dangling) == {main, second}

    # An instance whose constructor fails before registration is never enrolled.
    uninitialized = threading.Thread.__new__(threading.Thread)
    try:
        threading.Thread.__init__(uninitialized, group=object())
    except AssertionError:
        pass
    else:
        raise AssertionError("invalid group was accepted")
    assert uninitialized not in _dangling

    gate = threading.Lock()
    gate.acquire()
    running = threading.Thread(target=gate.acquire)
    running_ref = ref(running)
    running.start()
    try:
        assert running.is_alive() and _thread._count() == 1
        running.join(0)
        del running
        gc.collect()
        # A live thread is retained even with only its weak reference in this scope.
        stopped = running_ref()
        assert stopped is not None and stopped in _dangling
    finally:
        gate.release()
    stopped.join(5)
    assert not stopped.is_alive() and _thread._count() == 0
    assert set(_dangling) == {main, second, stopped}
    del second, stopped
    gc.collect()
    assert running_ref() is None and set(_dangling) == {main}


check()
