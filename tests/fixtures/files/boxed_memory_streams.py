from io import BufferedIOBase, BytesIO, IOBase, RawIOBase, StringIO, TextIOBase
from typing import BinaryIO, TextIO, cast


def environment(payload: bytes, errors: StringIO) -> dict[str, object]:
    return {
        "PATH_INFO": "/echo",
        "wsgi.version": (1, 0),
        "wsgi.input": BytesIO(payload),
        "wsgi.errors": errors,
        "wsgi.multithread": False,
    }


def input_stream(env: dict[str, object]) -> BytesIO:
    value = env["wsgi.input"]
    if isinstance(value, BytesIO):
        return cast(BytesIO, value)
    raise TypeError("expected a byte stream")


def write_error(env: dict[str, object], text: str) -> None:
    value = env["wsgi.errors"]
    if isinstance(value, StringIO):
        print(value.write(text))
        return
    raise TypeError("expected a text stream")


def binary_class(value: BinaryIO) -> None:
    print((isinstance(value, BytesIO), isinstance(value, StringIO)))


def text_class(value: TextIO) -> None:
    print((isinstance(value, BytesIO), isinstance(value, StringIO)))


def mixed_storage(file: BinaryIO) -> None:
    memory = BytesIO(b"ab")
    streams = [memory, file]
    table = {"memory": memory, "file": file}
    print((streams[0].read(1), table["memory"].read(), streams[1] is file))
    alias = BytesIO()
    print(isinstance(alias, BytesIO))
    alias = file
    print((alias is file, isinstance(alias, BytesIO)))


errors = StringIO()
env = environment(b"abcdef", errors)
stream = input_stream(env)
raw = env["wsgi.input"]
print((raw is stream, stream is raw, raw == stream, stream == raw))
print((raw is errors, env["wsgi.errors"] is errors, env["wsgi.errors"] == errors))
print((raw == BytesIO(b"abcdef"), stream is errors))
print((isinstance(raw, IOBase), isinstance(raw, BufferedIOBase), isinstance(raw, RawIOBase)))
print((isinstance(raw, TextIOBase), isinstance(env["wsgi.errors"], TextIOBase)))
print((repr(raw).startswith("<_io.BytesIO object at 0x"), repr(env["wsgi.errors"]).startswith("<_io.StringIO object at 0x")))
print(stream.read(2))
print(input_stream(env).read(1))
write_error(env, "bad")
print(errors.getvalue())
env["wsgi.input"] = BytesIO(b"replacement")
print((input_stream(env) is stream, stream.read(), input_stream(env).tell()))
del env["wsgi.input"]
print((raw is stream, stream.seek(1), stream.read(2)))
stream.close()
print((isinstance(raw, BytesIO), bool(raw), stream.closed))
try:
    stream.read()
except ValueError as e:
    print(str(e))
errors.close()
try:
    write_error(env, "closed")
except ValueError as e:
    print(str(e))
env["wsgi.input"] = "wrong"
print(isinstance(cast(BytesIO, env["wsgi.input"]), str))
try:
    input_stream(env)
except TypeError as e:
    print(str(e))
binary_class(stream)
text_class(errors)
with open("concrete-stream.bin", "wb") as binary_file:
    binary_class(binary_file)
    mixed_storage(binary_file)
with open("concrete-stream.bin", "r", encoding="utf-8") as text_file:
    text_class(text_file)
