import codecs
import io
import logging
from pathlib import Path


def exercise(handler: logging.StreamHandler, source: str) -> None:
    stream = handler.stream
    if stream is None:
        raise AssertionError("missing stream")
    for _ in range(2):
        try:
            stream.write(source)
            print("continued after failed write")
        except UnicodeEncodeError as exc:
            print(exc.args[1] is source)
            print(str(exc))
        else:
            raise AssertionError("unencodable input")
        print(stream.write("A"))
    handler.close()


path = Path("handler.txt")
with path.open("w", encoding="iso8859-15") as file:
    exercise(logging.StreamHandler(file), "x😀y")
print(path.read_bytes())
raw = io.BytesIO()
writer = codecs.getwriter("iso8859-15")(raw)
exercise(logging.StreamHandler(writer), "x😀y")
print(raw.getvalue())
