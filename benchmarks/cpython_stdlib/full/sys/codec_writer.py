"""Observe writer class mutations and the bytes delivered to the original stream."""
import codecs
import io


def markers() -> None:
    cls = codecs.getwriter("utf-8-sig")
    stream = io.BytesIO()
    writer = cls(stream)
    writer.write("")
    writer.write("x")
    writer.reset()
    writer.write("Y")
    print(repr(stream.getvalue()))
    stream2 = io.BytesIO()
    other = cls(errors="strict", stream=stream2)
    print(writer is other)
    other.write("z")
    print(repr(stream2.getvalue()))


def main() -> None:
    cls = codecs.getwriter("cp1251")
    alias = codecs.getwriter(encoding="windows-1251")
    print(alias is cls)
    print(cls is codecs.lookup("1251").streamwriter)
    print(cls is codecs.getwriter("latin1"))
    try:
        print(cls.encoding)
    except AttributeError as error:
        print(str(error))
    cls.encoding = "cp1251"
    print(alias.encoding)
    stream = io.BytesIO()
    writer = cls(stream, "strict")
    print(writer.stream is stream)
    print(writer.errors)
    print(writer.write("до свидания\n"))
    writer.flush()
    print(repr(stream.getvalue()))
    try:
        writer.write("A☃B")
    except UnicodeEncodeError as error:
        print(str(error))
    print(repr(stream.getvalue()))
    alias.encoding = "a class label"
    print(cls.encoding)
    writer2 = codecs.getwriter("1251")(stream, "replace")
    writer2.write("А☃")
    print(repr(stream.getvalue()))
    writer.close()
    print(stream.closed)
    try:
        writer2.write("Я")
    except ValueError as error:
        print(str(error))
    try:
        codecs.getwriter("does-not-exist")
    except LookupError as error:
        print(str(error))
    markers()


main()
