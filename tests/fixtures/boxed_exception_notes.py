def current():
    class Note(str):
        pass

    class Notes(list):
        def append(self, note):
            raise AssertionError("list.append override called")

    note = Note("first")
    error = ValueError("problem")
    assert not hasattr(error, "__notes__")
    assert error.add_note(note) is None
    assert error.__dict__["__notes__"][0] is note
    notes = Notes()
    error.__notes__ = notes
    try:
        raise error
    except ValueError as caught:
        caught.add_note("second")
        assert caught is error and caught.__notes__ is notes
    assert notes == ["second"]
    del error.__notes__
    assert not hasattr(error, "__notes__")
    error.add_note = lambda text: text
    assert error.add_note(note) is note
    BaseException.add_note(error, note)
    assert error.__notes__ == [note]

    error.__notes__ = None
    for value in (None, 1, ValueError, "valid"):
        try:
            BaseException.add_note(error, value)
        except TypeError as caught:
            print(str(caught))
        else:
            raise AssertionError(value)

    class Hooked(Exception):
        def __getattribute__(self, name):
            if name == "__notes__":
                events.append(name)
                raise AttributeError(name)
            return object.__getattribute__(self, name)

        def __setattr__(self, name, value):
            events.append(name)
            assigned.append(value)

    events = []
    assigned = []
    e = Hooked()
    e.add_note(note)
    assert events == ["__notes__", "__notes__"] and events[0] is events[1]
    assert assigned[0][0] is note
    assert e.__dict__ == {}

    class WriteOnly:
        def __set__(self, instance, value):
            raise AssertionError("shadowed descriptor setter called")

    class Preseeded(Exception):
        __notes__ = WriteOnly()

    preseeded = Preseeded()
    assert preseeded.add_note.__qualname__ == Preseeded.__qualname__ + ".add_note"
    preseeded.__dict__["__notes__"] = notes
    preseeded.add_note(note)
    assert notes[-1] is note

    class Mixin(BaseException):
        def add_note(self, text):
            return text

    class Mixed(ValueError, Mixin):
        def inherited(self, text):
            return super().add_note(text)

    mixed = Mixed()
    assert mixed.add_note(note) is note and mixed.inherited(note) is note
    assert not hasattr(mixed, "__notes__")
