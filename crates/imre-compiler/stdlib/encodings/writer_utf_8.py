# CPython 3.14.5, Lib/encodings/utf_8.py.
import codecs


class StreamWriter(codecs.StreamWriter):
    encode = codecs.utf_8_encode
