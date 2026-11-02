def current():
    import json

    class Array(list):
        pass

    class Values(tuple):
        def __iter__(self):
            return iter((4, 3))

    class Record(dict):
        def items(self):
            return [("b", 2), ("a", 1)]

    def reject(value):
        raise AssertionError("unexpected default")

    assert json.dumps(Array([1, 2]), default=reject) == "[1, 2]"
    assert json.dumps([Values((9,))]) == "[[4, 3]]"
    assert json.dumps(Record(key=0), sort_keys=True) == '{"a": 1, "b": 2}'
    class Shrinking(list):
        def __iter__(self):
            if self:
                self.clear()
                return iter((self,))
            return iter(())

    assert json.dumps(Shrinking([1])) == "[[]]"
    recursive = Array()
    recursive.append(recursive)
    try:
        json.dumps(recursive)
    except ValueError:
        pass
    else:
        raise AssertionError("circular reference")
    class MixedKeys(dict):
        def items(self):
            return [(b"omit", object()), ("kept", [{b"omit": 0}]), ((), object())]

    from io import StringIO
    mixed = MixedKeys(placeholder=True)
    expected = '{\n  "kept": [\n    {}\n  ]\n}'
    assert json.dumps(mixed, skipkeys=True, indent=2, default=reject) == expected
    stream = StringIO()
    json.dump(mixed, stream, skipkeys=True, indent=2, default=reject)
    assert stream.getvalue() == expected

    # Sorting observes the original keys, including ones that would be skipped.
    try:
        json.dumps({"a": 1, b"b": 2}, sort_keys=True, skipkeys=True)
    except TypeError:
        pass
    else:
        raise AssertionError("incomparable keys")

    print("encoded container subclasses")
