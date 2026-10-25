"""Exercise the codec class, the wrapped binary stream and their distinct state."""

import codecs
import io


def run():
    cls = codecs.getwriter("cp1251")
    print(cls is codecs.getwriter("windows-1251"))
    print(cls is codecs.lookup("1251").streamwriter)
    print(codecs.lookup("1251").name)
    first = io.BytesIO()
    second = io.BytesIO()
    writer = cls(first, "strict")
    other = cls(stream=second, errors="replace")
    try:
        print(cls.encoding)
    except AttributeError as exc:
        print(str(exc))
    try:
        print(writer.encoding)
    except AttributeError as exc:
        print(str(exc))
    cls.encoding = "cp1251"
    print(writer.encoding)
    print(writer.stream is first)
    print(writer.errors)
    print(writer.write("до свидания\n") is None)
    writer.flush()
    saved = first.getvalue()
    print(saved)
    print(first.tell())
    try:
        writer.write("partial☃☃")
    except UnicodeEncodeError as exc:
        print(str(exc))
    print(first.getvalue())
    other.write("Ё☃€")
    print(second.getvalue())
    writer.encoding = "independent metadata"
    cls.encoding = "shared metadata"
    print(writer.encoding)
    print(other.encoding)
    writer.errors = "backslashreplace"
    writer.write("☃")
    print(first.getvalue())
    print(saved)
    other.close()
    print(second.closed)
    print(first.closed)
    try:
        print(second.getvalue())
    except ValueError as exc:
        print(str(exc))
    try:
        other.write("closed")
    except ValueError as exc:
        print(str(exc))
    bom_stream = io.BytesIO()
    sig = codecs.getwriter("utf-8-sig")(bom_stream)
    sig.write("")
    sig.write("x")
    sig.reset()
    sig.write("y")
    print(bom_stream.getvalue())


run()
