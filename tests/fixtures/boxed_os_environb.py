import os
import posix
import sys


def current():
    assert os.supports_bytes_environ is True
    encoding = sys.getfilesystemencoding()
    raw = b"IMRE_ENVIRONB_\xff"
    key = raw.decode(encoding, "surrogateescape")
    original = os.environb.get(raw)
    try:
        os.environb[raw] = b"\xff"
        assert os.environ[key].encode(encoding, "surrogateescape") == b"\xff"
        assert posix.environ[raw] == b"\xff"
        assert raw in list(os.environb)
        snapshot = os.environb.copy()
        os.environ[key] = "from text"
        assert snapshot[raw] == b"\xff"
        assert os.getenvb(raw) == b"from text"
        os.environb.update({raw: b"from bytes"})
        assert os.getenv(key) == "from bytes"

        for view, name, value, replacement in (
            (os.environb, raw, b"from bytes", b"replacement"),
            (os.environ, key, "from bytes", "replacement"),
        ):
            assert repr(view) == f"environ({view.copy()!r})"
            assert (view | {name: replacement})[name] == replacement
            assert ({name: replacement} | view)[name] == value
            assert view.setdefault(name, replacement) == value
            del view[name]
            assert view.setdefault(name, replacement) == replacement
            original_view = view
            view |= [(name, value)]
            assert view is original_view and view[name] == value

        for invalid in ("text", bytearray(b"bytes"), None):
            try:
                os.environb[raw] = invalid
            except TypeError:
                pass
            else:
                raise AssertionError("accepted a non-bytes value")
            try:
                os.getenvb(invalid)
            except TypeError:
                pass
            else:
                raise AssertionError("accepted a non-bytes key")

        for bad_key, bad_value in ((raw + b"=", b"x"), (raw + b"\0", b"x"), (raw, b"v\0")):
            try:
                os.environb[bad_key] = bad_value
            except ValueError:
                pass
            else:
                raise AssertionError("accepted invalid POSIX environment data")
        assert os.environb[raw] == b"from bytes"
        del os.environ[key]
        assert raw not in posix.environ
        fallback = object()
        assert os.getenvb(raw, fallback) is fallback
        try:
            os.environb[raw]
        except KeyError as err:
            assert err.args == (raw,)
        else:
            raise AssertionError("missing byte key returned a value")
    finally:
        os.environb.pop(raw, None)
        if original is not None:
            os.environb[raw] = original
