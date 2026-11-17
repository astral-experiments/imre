import _thread
import gc
import threading
import weakref


class Worker(threading.Thread):
    def run(self):
        self.gate.acquire()


def ignored(*args, **kwargs):
    raise AssertionError("an overridden run called the target")


def replacement(self):
    raise AssertionError("run was looked up again")


gate = _thread.allocate_lock()
gate.acquire()
args = (object(),)
kwargs = {"value": object()}
worker = Worker(target=ignored, args=args, kwargs=kwargs)
worker.gate = gate
function_ref = weakref.ref(Worker.run)
owner_ref = weakref.ref(worker)
lock_ref = weakref.ref(gate)
worker.start()
print("override alive:", worker.is_alive())
try:
    worker.start()
except RuntimeError:
    pass
else:
    raise AssertionError("a thread was started twice")
Worker.run = replacement
worker.gate = _thread.allocate_lock()
del worker, gate
gc.collect()
assert function_ref() is not None and owner_ref() is not None and lock_ref() is not None
worker = owner_ref()
lock_ref().release()
worker.join(3)
print("override stopped:", not worker.is_alive())
print("override fields:", worker._target is ignored, worker._args is args, worker._kwargs is kwargs)
assert not worker.is_alive()
gc.collect()
assert function_ref() is None and lock_ref() is None

default = threading.Thread(target=_thread.allocate_lock().acquire)
default.start()
default.join(3)
assert not default.is_alive()
print("default fields:", *(hasattr(default, name) for name in ("_target", "_args", "_kwargs")))
