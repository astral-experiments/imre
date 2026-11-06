MISSING = object()


class Label:
    def __init__(self, value: str = MISSING, /, *, name: str = MISSING):
        self.value = "missing" if value is MISSING else value
        self.name = "missing" if name is MISSING else name


def required(value: str, *, name):
    return value, name


def current():
    omitted = Label()
    assert (omitted.value, omitted.name) == ("missing", "missing")
    assert Label("first").value == "first"
    assert Label(name="second").name == "second"
    assert Label("first", name="second").name == "second"
    try:
        required(1)
    except TypeError as exc:
        assert "required keyword-only argument" in str(exc)
    else:
        raise AssertionError("missing keyword-only argument")
