def current():
    from _json import encode_basestring, encode_basestring_ascii

    class Text(str):
        def __str__(self):
            raise AssertionError("must use the stored text")

    class xéééééééééééééééééééééééééééééééééééééééééééééééééé:
        pass

    text = Text('"\t\\\x00\x1f\x7fé𝄞\ud800a\udc00')
    prefix = '"\\"\\t\\\\\\u0000\\u001f'
    raw = encode_basestring(text)
    assert type(raw) is str and raw == prefix + '\x7fé𝄞\ud800a\udc00"'
    assert encode_basestring_ascii(text) == prefix + '\\u007f\\u00e9\\ud834\\udd1e\\ud800a\\udc00"'
    for quote in (encode_basestring, encode_basestring_ascii):
        assert quote.__module__ == "_json"
        assert quote("") == '""'
        try:
            quote(b"x")
        except TypeError as error:
            assert str(error) == "first argument must be a string, not bytes"
        else:
            raise AssertionError("accepted bytes")
        try:
            quote(xéééééééééééééééééééééééééééééééééééééééééééééééééé())
        except TypeError as error:
            assert str(error) == "first argument must be a string, not x" + "é" * 39
        else:
            raise AssertionError("accepted non-string instance")
        try:
            quote(s="x")
        except TypeError as error:
            assert str(error) == "_json." + quote.__name__ + "() takes no keyword arguments"
        else:
            raise AssertionError("accepted a keyword")
    print("native JSON string quoting")
