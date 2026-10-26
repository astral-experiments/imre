import codecs
import io
import logging


def show(handler: logging.StreamHandler) -> None:
    stream = handler.stream
    if stream is not None:
        print(stream.errors)
        try:
            print(stream.encoding)
        except AttributeError as exc:
            print(str(exc))


def write(handler: logging.StreamHandler) -> None:
    stream = handler.stream
    if stream is not None:
        print(stream.write(""))
        print(stream.write("a"))


def run() -> None:
    handler = logging.StreamHandler(io.StringIO())
    show(handler)
    write(handler)
    with open("text.txt", "w", encoding="utf-8") as output:
        handler.setStream(output)
        show(handler)
        write(handler)
    show(handler)

    writer = codecs.getwriter("cp1251")(io.BytesIO(), "ignore")
    handler = logging.StreamHandler(writer)
    show(handler)
    write(handler)
    writer.encoding = "instance"
    writer.errors = "replace"
    show(handler)
    handler.close()


run()
