import sys
import threading


class Owner:
    def __init__(self, lock, seen):
        self.lock = lock
        self.seen = seen

    def acquire(self):
        self.lock.acquire()

    def enter(self):
        self.lock.__enter__()

    def release(self):
        self.lock.release()

    def effect(self):
        self.seen.append('effect')

    def prefixed(self):
        self.effect()
        self.lock.acquire()


class Impostor:
    def __init__(self, seen):
        self.seen = seen

    def __getattribute__(self, name):
        if name == 'acquire':
            object.__getattribute__(self, 'seen').append('lookup')
        return object.__getattribute__(self, name)

    def acquire(self):
        self.seen.append('called')


class Typed:
    def __init__(self, lock):
        self.lock = lock

    def acquire(self: Typed):
        self.lock.acquire()


def start(target):
    thread = threading.Thread(target=target)
    try:
        thread.start()
    except NotImplementedError:
        assert thread.ident is None and thread.native_id is None
        return False
    thread.join(2)
    assert not thread.is_alive()
    return True


def check():
    seen = []
    lock = threading.Lock()
    owner = Owner(lock, seen)
    assert start(lock.acquire) and lock.locked()
    lock.release()
    assert start(owner.acquire) and lock.locked()
    lock.release()

    # Both written selectors resolve to the same registered Acquire operation.
    assert start(lock.__enter__) and lock.locked()
    lock.release()
    assert start(owner.enter) and lock.locked()
    lock.release()

    native = sys.implementation.name == "imre"
    started = start(owner.prefixed)
    assert started != native
    assert seen == (['effect'] if started else [])
    assert lock.locked() == started
    if lock.locked(): lock.release()
    seen.clear()

    owner.lock = Impostor(seen)
    started = start(owner.acquire)
    assert started != native
    assert seen == (['lookup', 'called'] if started else [])
    owner.lock = lock
    seen.clear()

    lock.acquire()
    started = start(owner.release)
    assert started != native
    assert lock.locked() != started
    if lock.locked(): lock.release()

    valid = Typed(lock)
    assert start(valid.acquire) and lock.locked()
    lock.release()
    target = Typed.acquire.__get__(owner)
    assert start(target) and lock.locked()
    lock.release()

    # User-class annotations are advisory. A published scalar annotation is an
    # enforced Imre contract, both for ordinary binding and for worker admission.
    target = valid.acquire
    annotations = Typed.acquire.__annotations__
    original = annotations['self']
    annotations['self'] = int
    try:
        try:
            target()
        except TypeError:
            assert native and not lock.locked()
        else:
            assert not native and lock.locked()
            lock.release()
        started = start(target)
        assert started != native and lock.locked() == started
        if lock.locked(): lock.release()
    finally:
        annotations['self'] = original


check()
