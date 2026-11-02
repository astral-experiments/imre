from array import array


class Data(bytes):
    pass


def current():
    assert Data(b"abc").endswith(b"bc")
    for method, wrong in ((bytes.endswith, bytearray(b"a")), (bytearray.endswith, b"a")):
        assert type(method).__name__ == "method_descriptor"
        assert method.__text_signature__ == "($self, suffix[, start[, end]], /)"
        try:
            method.__get__(wrong)
        except TypeError as error:
            msg = f"descriptor 'endswith' for '{method.__objclass__.__name__}' objects doesn't apply to a '{type(wrong).__name__}' object"
            assert str(error) == msg
        else:
            raise AssertionError("invalid receiver bound")
    empty_word = array("i", [0])
    for value in (empty_word.tobytes(), bytearray(empty_word.tobytes())):
        assert value.endswith((empty_word, 4))
    for value in (b"abc", bytearray(b"abc")):
        assert value.endswith((b"x", bytearray(b"bc"), 3))
        assert value.endswith(b"b", 0, -1)
        assert not value.endswith(b"", 4)
        assert not value.endswith(())
        try:
            value.endswith((b"x", "c"))
        except TypeError as error:
            assert str(error) == "a bytes-like object is required, not 'str'"
        else:
            raise AssertionError("invalid tuple member was accepted")
    print("binary suffix checks passed")
