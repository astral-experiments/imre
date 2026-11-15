import hashlib
import threading
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
