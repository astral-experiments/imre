import gc
import weakref


events = []


class Key:
    def __hash__(self):
        return 31

    def __del__(self):
        events.append("finalized")


def retained_result():
    key = Key()
    reference = weakref.ref(key, lambda r: events.append("weak"))
    receiver = frozenset([key])
    result = receiver.intersection([key], (key,))
    del key
    del receiver
    gc.collect()
    assert reference() is next(iter(result))
    assert events == []
    del result
    gc.collect()
    assert reference() is None
    assert sorted(events) == ["finalized", "weak"]


def release_iterator():
    history = []

    def stream():
        try:
            yield 1
            raise AssertionError("intersection should stop once complete")
        finally:
            history.append("closed")

    def next_operand():
        history.append("next")
        yield 1

    assert {1}.intersection(stream(), next_operand()) == {1}
    assert history == ['next', 'closed']


def current():
    events.clear()
    retained_result()
    release_iterator()
