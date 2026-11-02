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

    # A Python string can contain both raw surrogate halves and scalar values.
    key = "é\ud800\udfff"
    value = '\ufffd"\n\udc0d'
    doc = {key: [value]}
    escaped = '{"\\u00e9\\ud800\\udfff": ["\\ufffd\\"\\n\\udc0d"]}'
    raw = '{"é\ud800\udfff": ["\ufffd\\"\\n\udc0d"]}'
    for ascii, want in ((True, escaped), (False, raw)):
        assert json.dumps(doc, ensure_ascii=ascii, skipkeys=True) == want
        stream = StringIO()
        json.dump(doc, stream, ensure_ascii=ascii)
        assert stream.getvalue() == want

    chunks = []
    class Writer:
        def write(self, chunk):
            chunks.append(chunk)
            if chunk == '"\udfff"':
                raise ValueError("writer stopped")

    try:
        json.dump({key: '\udfff', 'other': object()}, Writer(), ensure_ascii=False, default=reject)
    except ValueError as error:
        assert str(error) == "writer stopped"
        assert not hasattr(error, "__notes__")
    else:
        raise AssertionError("writer exception")
    assert chunks == ['{', '"' + key + '"', ': ', '"\udfff"']

    class Key:
        @property
        def __class__(self):
            return Record

    for streaming, expected_name in [(False, "Key"), (True, "Record")]:
        output = StringIO()
        try:
            data = {"first": 1, Key(): 2}
            if streaming:
                json.dump(data, output)
            else:
                json.dumps(data)
        except TypeError as exc:
            assert exc.args == ("keys must be str, int, float, bool or None, not " + expected_name,)
        else:
            raise AssertionError("invalid key")
        if streaming:
            assert output.getvalue() == '{"first": 1'

    class Failure(Exception):
        def add_note(self, note):
            calls.append(note)
            super().add_note(note)

    def fail(value):
        raise original

    for streaming in (False, True):
        original = Failure("failed to encode value")
        calls = []
        notes = ["existing"]
        original.__notes__ = notes
        wrapped = [{23: object()}]
        try:
            if streaming:
                json.dump(wrapped, StringIO(), default=fail)
            else:
                json.dumps(wrapped, default=fail)
        except Failure as caught:
            assert caught is original and caught.__notes__ is notes
        else:
            raise AssertionError("expected callback error")
        key = "'23'" if streaming else "23"
        expected = ["when serializing dict item " + key, "when serializing list item 0"]
        assert notes == ["existing"] + expected
        assert calls == (expected if streaming else [])

    # The default result loses the encoder's reference before the next item.
    row = [object(), "initial"]
    retained = []
    class Replacement(str):
        def __del__(self):
            row[1] = "updated"

    def default(obj):
        result = Replacement("first")
        if keep:
            retained.append(result)
        return result

    for keep, second in [(False, "updated"), (True, "initial")]:
        row[1] = "initial"
        assert json.dumps(row, default=default) == '["first", "' + second + '"]'
        retained.clear()
        assert row[1] == "updated"

    print("encoded container subclasses")
