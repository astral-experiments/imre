import os
import tempfile
from tempfile import NamedTemporaryFile


def current():
    source = NamedTemporaryFile(dir=".", prefix="named-", suffix=".bin")
    name = source.name
    with source as alias:
        assert alias is source
        assert name.endswith(".bin") and os.path.isfile(name)
        assert not os.get_inheritable(alias.fileno())
        assert alias.write(b"first\0\xff") == 7
        alias.flush()
        with open(name, "rb") as reopened:
            assert reopened.read() == b"first\0\xff"
        alias.seek(0)
        assert alias.read(5) == b"first"
        source.file.close()
        assert alias.closed and os.path.exists(name)
        alias.close()
        assert not os.path.exists(name)
        # A second close, including context exit, must leave a replacement alone.
        with open(name, "wb") as replacement:
            replacement.write(b"replacement")
        source.close()
    with open(name, "rb") as replacement:
        assert replacement.read() == b"replacement"

    kept = NamedTemporaryFile(dir=".")
    kept_name = kept.name
    write = kept.write
    kept = None
    assert os.path.exists(kept_name)
    assert write(b"still open") == 10
    write = None
    assert not os.path.exists(kept_name)

    deferred = NamedTemporaryFile(dir=".", delete_on_close=False)
    deferred_name = deferred.name
    deferred.close()
    assert os.path.exists(deferred_name)
    del deferred
    assert not os.path.exists(deferred_name)

    for delete in (True, False):
        for on_close in (True, False):
            with NamedTemporaryFile("wb+", dir=".", delete=delete,
                                    delete_on_close=on_close) as tmp:
                tmp.close()
                assert tmp.closed
                assert os.path.exists(tmp.name) == (not (delete and on_close))
            tmp.close()
            assert os.path.exists(tmp.name) == (not delete)

    try:
        with NamedTemporaryFile(dir=".", delete_on_close=False) as interrupted:
            raise ValueError("body")
    except ValueError as error:
        assert str(error) == "body"
    assert interrupted.closed and not os.path.exists(interrupted.name)

    try:
        NamedTemporaryFile(prefix="text", dir=b".")
    except TypeError:
        pass
    else:
        raise AssertionError("mixed str and bytes parameters")
    try:
        NamedTemporaryFile(dir="missing-named-tmp-parent")
    except FileNotFoundError:
        pass
    else:
        raise AssertionError("missing parent")
    # Fail after exclusive creation, even when successful files would be retained.
    saved = []

    def interrupt(stream, name, *policy):
        saved.append((stream, name))
        raise ValueError("wrapper")

    original = tempfile._TemporaryFileWrapper
    tempfile._TemporaryFileWrapper = interrupt
    try:
        for delete in (False, True):
            try:
                NamedTemporaryFile(dir=".", delete=delete)
            except ValueError as error:
                assert str(error) == "wrapper"
            else:
                raise AssertionError("constructor error was lost")
            stream, name = saved[-1]
            assert stream.closed and not os.path.exists(name)
    finally:
        tempfile._TemporaryFileWrapper = original

    # The comparison runner removes the replacement and both delete=False files.
    print("NamedTemporaryFile lifecycle checked")
