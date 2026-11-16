import _thread
import threading


def make_wait(gate, decoy):
    class Listener:
        gate = decoy

        @staticmethod
        def wait():
            gate.acquire()

    return Listener.wait


def check():
    gate = _thread.allocate_lock()
    decoy = _thread.allocate_lock()
    gate.acquire()
    wait = make_wait(gate, decoy)
    assert wait.__code__.co_freevars == ("gate",)
    assert wait.__closure__[0].cell_contents is gate
    worker = threading.Thread(target=wait)
    worker.start()
    try:
        worker.join(0.01)
        assert worker.is_alive()
        assert worker.native_id != threading.get_native_id()
        assert not decoy.locked()
    finally:
        gate.release()
        worker.join(5)
    assert gate.locked() and not worker.is_alive()


check()
print("joined method with lexical lock")
