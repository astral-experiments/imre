class Text(str):
    pass


def current():
    tail = "\udcff"
    path = "/home/" + tail + "//"
    assert path.rstrip("/") == "/home/" + tail
    assert path.lstrip("/") == "home/" + tail + "//"
    assert path.strip("/") == "home/" + tail
    assert (tail + "middle" + tail).strip(Text(tail)) == "middle"
    assert "/middle/".strip(tail + "/") == "middle"
    assert (tail + "\ufffd").rstrip(tail) == tail + "\ufffd"
    assert ("\x1c" + tail + "\u2003").strip() == tail
    assert (tail + "\t").rstrip(None) == tail
    assert Text(tail).strip(tail) == ""

    for name in ("strip", "lstrip", "rstrip"):
        for value in (path, "unchanged"):
            assert getattr(value, name)("") is value
        for value in (Text(tail), Text("x")):
            method = getattr(value, name)
            result = method()
            assert result == value and type(result) is str and result is not method()
        for args, kwargs, message in (
            ((b"/",), {}, name + " arg must be None or str"),
            (("/", "/"), {}, name + " expected at most 1 argument, got 2"),
            ((), {"chars": "/"}, "Text." + name + "() takes no keyword arguments"),
        ):
            try:
                getattr(Text(path), name)(*args, **kwargs)
            except TypeError as exc:
                assert str(exc) == message
            else:
                raise AssertionError((name, args, kwargs))
    print("string strip matched")
