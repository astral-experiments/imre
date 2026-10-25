import codecs
import io


def label(value: str, /) -> str:
    return value


codec = codecs.getwriter("cp1251")
codec(stream=io.BytesIO(), errors=label(value="strict"))
