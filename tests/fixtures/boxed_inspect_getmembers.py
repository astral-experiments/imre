from inspect import getmembers, getmembers_static


class Members:
    def __init__(self):
        self.reads = 0

    def __dir__(self):
        return ["value", "missing", "method", "value"]

    def method(self):
        pass

    @property
    def value(self):
        self.reads += 1
        return 42


def reject(value):
    raise AttributeError("predicate failed")


def current():
    subject = Members()
    assert getmembers_static(subject) == [("method", Members.method), ("value", Members.value)]
    assert subject.reads == 0
    assert getmembers(subject) == [("method", subject.method), ("value", 42)]
    assert subject.reads == 2
    try:
        getmembers_static(subject, reject)
    except AttributeError as exc:
        assert exc.args == ("predicate failed",)
    else:
        raise AssertionError("predicate exception was swallowed")


if __name__ == "__main__":
    current()
