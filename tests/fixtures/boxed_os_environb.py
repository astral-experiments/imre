import os
import posix
import subprocess
import sys


def check_popen():
    key = "IMRE_POPEN_ENVIRON_PROBE"
    old = os.environ.get(key)
    try:
        os.environ[key] = "mapping value"
        # putenv changes what children inherit without updating os.environ.
        os.putenv(key, "inherited value")
        command = "printf '%s\\n' first \"$IMRE_POPEN_ENVIRON_PROBE\""
        result = subprocess.run(
            ["/bin/sh", "-c", command], env=os.environ, capture_output=True, encoding="utf-8", check=True
        )
        assert result.stdout == "first\nmapping value\n"
        with os.popen(command) as reader:
            assert next(iter(reader)) == "first\n"
            assert reader.read() == "inherited value\n"
        assert reader.closed

        writer = os.popen('IFS= read -r word && [ "$word" = "$IMRE_POPEN_ENVIRON_PROBE" ]', "w")
        writer.write("inherited value\n")
        assert writer.close() is None
        assert os.popen("exit 7").close() == 7 << 8
    finally:
        if old is None:
            os.environ.pop(key, None)
        else:
            os.environ[key] = old


def current():
    check_popen()
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

        text_view, bytes_view, data = os.environ, os.environb, posix.environ
        os.putenv(raw, b"\xfe")
        assert bytes_view[raw] == b"from bytes"
        snapshot = posix._create_environ()
        assert snapshot[raw] == b"\xfe"
        snapshot[raw] = b"local"
        assert os.reload_environ() is None
        assert os.environ is text_view and os.environb is bytes_view
        assert posix.environ is data and data[raw] == b"\xfe"
        assert text_view[key].encode(encoding, "surrogateescape") == b"\xfe"
        os.unsetenv(raw)
        assert raw in bytes_view
        os.reload_environ()
        assert raw not in data and key not in text_view and raw not in bytes_view

        os.environb[raw] = b"from bytes"
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
