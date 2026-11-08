import gc


saved = []


class Batch:
    def __del__(self):
        stream = self.stream
        saved.append((stream, next(stream, None)))


def receipts(owners):
    yield "queued"
    yield "posted"
    return owners


def queue(generator_first, started):
    owners = []
    if generator_first:
        stream = receipts(owners)
    batch = Batch()
    if not generator_first:
        stream = receipts(owners)
    owners.append(batch)
    batch.stream = stream
    if started:
        assert next(stream) == "queued"


for started, expected in ((False, "queued"), (True, "posted")):
    for generator_first in (False, True):
        queue(generator_first, started)
        gc.collect()
        stream, item = saved.pop()
        assert item == (None if generator_first else expected)
        assert stream.gi_frame is None
        assert next(stream, None) is None
