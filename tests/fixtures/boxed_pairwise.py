def current():
    from itertools import pairwise

    pulls = []

    class Input:
        def __iter__(self):
            pulls.append("iter")
            return self

        def __next__(self):
            pulls.append("next")
            if len(pulls) == 4:
                raise ValueError("stopped")
            return len(pulls)

    class Tagged(pairwise):
        def __init__(self, iterable, *, tag):
            self.tag = tag

        @property
        def _source(self):
            raise AssertionError("subclass intercepted the iterator")

    cursor = Tagged(Input(), tag=42)
    assert cursor.tag == 42 and pulls == ["iter"]
    assert iter(cursor) is cursor and next(cursor) == (2, 3)
    try:
        next(cursor)
    except ValueError as error:
        assert str(error) == "stopped"
    else:
        raise AssertionError("source exception was lost")
    assert next(cursor, "end") == "end" and pulls.count("next") == 3
