from __future__ import annotations
import codecs
import io
import sys
import typing as t
def _make_text_stream(
    stream: t.BinaryIO,
    encoding: str | None,
    errors: str | None,
    force_readable: bool = False,
    force_writable: bool = False,
) -> t.TextIO:
    if encoding is None:
        encoding = get_best_encoding(stream)
    if errors is None:
        errors = "replace"
    return _NonClosingTextIOWrapper(
        stream,
        encoding,
        errors,
        line_buffering=True,
        force_readable=force_readable,
        force_writable=force_writable,
    )

def is_ascii_encoding(encoding: str) -> bool:
    """Checks if a given encoding is ascii."""
    try:
        return codecs.lookup(encoding).name == "ascii"
    except LookupError:
        return False

def get_best_encoding(stream: t.IO[t.Any]) -> str:
    """Returns the default stream encoding if not found."""
    rv = getattr(stream, "encoding", None) or sys.getdefaultencoding()
    if is_ascii_encoding(rv):
        return "utf-8"
    return rv

class _NonClosingTextIOWrapper(io.TextIOWrapper):
    def __init__(
        self,
        stream: t.BinaryIO,
        encoding: str | None,
        errors: str | None,
        force_readable: bool = False,
        force_writable: bool = False,
        **extra: t.Any,
    ) -> None:
        self._stream = stream = t.cast(
            t.BinaryIO, _FixupStream(stream, force_readable, force_writable)
        )
        super().__init__(stream, encoding, errors, **extra)

    def __del__(self) -> None:
        try:
            self.detach()
        except Exception:
            pass

    def isatty(self) -> bool:
        # https://bitbucket.org/pypy/pypy/issue/1803
        return self._stream.isatty()

class _FixupStream:
    """The new io interface needs more from streams than streams
    traditionally implement.  As such, this fix-up code is necessary in
    some circumstances.

    The forcing of readable and writable flags are there because some tools
    put badly patched objects on sys (one such offender are certain version
    of jupyter notebook).
    """

    def __init__(
        self,
        stream: t.BinaryIO,
        force_readable: bool = False,
        force_writable: bool = False,
    ):
        self._stream = stream
        self._force_readable = force_readable
        self._force_writable = force_writable

    def __getattr__(self, name: str) -> t.Any:
        return getattr(self._stream, name)

    def read1(self, size: int) -> bytes:
        f = getattr(self._stream, "read1", None)

        if f is not None:
            return t.cast(bytes, f(size))

        return self._stream.read(size)

    def readable(self) -> bool:
        if self._force_readable:
            return True
        x = getattr(self._stream, "readable", None)
        if x is not None:
            return t.cast(bool, x())
        try:
            self._stream.read(0)
        except Exception:
            return False
        return True

    def writable(self) -> bool:
        if self._force_writable:
            return True
        x = getattr(self._stream, "writable", None)
        if x is not None:
            return t.cast(bool, x())
        try:
            self._stream.write(b"")
        except Exception:
            try:
                self._stream.write(b"")
            except Exception:
                return False
        return True

    def seekable(self) -> bool:
        x = getattr(self._stream, "seekable", None)
        if x is not None:
            return t.cast(bool, x())
        try:
            self._stream.seek(self._stream.tell())
        except Exception:
            return False
        return True

name=sys.argv[1]
if name == 'original_class_identity':
    assert _NonClosingTextIOWrapper.__bases__ == (io.TextIOWrapper,)
    assert issubclass(_NonClosingTextIOWrapper,io.TextIOWrapper)
    assert _FixupStream.__bases__ == (object,)
if name == 'write_line_buffering_and_detach':
    raw=io.BytesIO()
    wrapper=_make_text_stream(raw,"utf-8","strict")
    assert type(wrapper) is _NonClosingTextIOWrapper and isinstance(wrapper,io.TextIOWrapper)
    assert wrapper.encoding=="utf-8" and wrapper.errors=="strict" and wrapper.line_buffering
    assert wrapper.buffer._stream is raw and not wrapper.isatty()
    assert wrapper.write("é東\n")==3 and raw.getvalue()=="é東\n".encode("utf-8")
    wrapped=wrapper.detach()
    assert wrapped._stream is raw and not raw.closed
    try:
        wrapper.write("x")
    except ValueError as error:
        assert error.args==("underlying buffer has been detached",)
    else:
        raise AssertionError("detached wrapper accepted write")
    wrapper.__del__()
    assert not raw.closed
if name == 'read_seek_replace_and_newlines':
    raw=io.BytesIO(b"one\r\n\xff\rtwo\n")
    wrapper=_make_text_stream(raw,"utf-8","replace")
    assert wrapper.readline()=="one\n"
    assert wrapper.read()=="�\ntwo\n"
    assert wrapper.seek(0)==0 and wrapper.read()=="one\n�\ntwo\n"
    wrapper.detach()
    assert not raw.closed
if name == 'strict_unicode_error':
    raw=io.BytesIO(b"a\xffb")
    wrapper=_make_text_stream(raw,"utf-8","strict")
    try:
        wrapper.read()
    except UnicodeDecodeError as error:
        assert error.encoding=="utf-8" and error.object==b"a\xffb" and error.start==1 and error.end==2
    else:
        raise AssertionError("invalid byte was accepted")
    wrapper.detach()
    assert not raw.closed
if name == 'defaults_and_forced_wrapper':
    raw=io.BytesIO()
    wrapper=_make_text_stream(raw,None,None,force_readable=True,force_writable=True)
    assert wrapper.errors=="replace" and wrapper.encoding=="utf-8"
    assert wrapper._stream.readable() and wrapper._stream.writable() and wrapper._stream.seekable()
    wrapper.write("雪")
    wrapper.flush()
    assert raw.getvalue()=="雪".encode("utf-8")
    wrapper.detach()
if name == 'supplied_binary_read1_fallback':
    class Reader:
        def __init__(self,raw):
            self.raw=raw
        def read(self,size=-1):
            return self.raw.read(size)
        def writable(self):
            return False
        def seekable(self):
            return False
        @property
        def closed(self):
            return self.raw.closed
        def flush(self):
            return None
    raw=io.BytesIO("é東\n".encode("utf-8")); source=Reader(raw)
    wrapper=_make_text_stream(source,"utf-8","strict")
    assert wrapper.readline()=="é東\n" and wrapper.read()==""
    assert wrapper.detach()._stream is source and not raw.closed
if name == 'drop_keeps_original_buffer_open':
    raw=io.BytesIO()
    wrapper=_make_text_stream(raw,"utf-8","strict")
    wrapper.write("kept")
    del wrapper
    assert not raw.closed and raw.getvalue()==b"kept"
    assert raw.write(b"!")==1 and raw.getvalue()==b"kept!"
print(name)
