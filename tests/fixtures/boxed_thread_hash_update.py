import hashlib
import threading
import gc
from multiprocessing.dummy import Process


def check(algorithm, thread_type=threading.Thread):
    chunk = b"abcdefgh" * 4096
    digest = hashlib.new(algorithm)
    before = digest.copy()
    workers = [thread_type(target=digest.update, args=(chunk,)) for _ in range(5)]
    for worker in workers:
        worker.start()
    for worker in workers:
        worker.join(3)
        assert not worker.is_alive()
        assert worker.ident != threading.get_ident()
    assert digest.digest() == hashlib.new(algorithm, chunk * len(workers)).digest()
    assert before.digest() == hashlib.new(algorithm).digest()


check("sha1")
check("blake2b")
check("sha256", Process)


class Writer:
    def __init__(self, state, chunk):
        self.state = state
        self.chunk = chunk

    def update(self):
        self.state.update(self.chunk)


def replacement(self):
    raise AssertionError("a saved bound target was looked up again")


def check_projected():
    first = hashlib.new("sha1")
    selected = hashlib.new("sha1")
    alias = selected
    selected.update(b"creator")
    writer = Writer(first, b"old")
    target = writer.update
    writer.state = selected
    writer.chunk = b"worker"
    original = Writer.update
    Writer.update = replacement
    try:
        worker = threading.Thread(target=target)
        worker.start()
        gc.collect()
        worker.join(3)
        assert not worker.is_alive()
    finally:
        Writer.update = original
    assert alias is selected
    assert selected.digest() == hashlib.new("sha1", b"creatorworker").digest()
    assert first.digest() == hashlib.new("sha1").digest()


check_projected()


def check_ordinary_buffers():
    state = hashlib.new("sha1")
    mutable = bytearray(b"abc")
    state.update(mutable)
    state.update(memoryview(mutable))
    assert state.digest() == hashlib.new("sha1", b"abcabc").digest()


check_ordinary_buffers()
