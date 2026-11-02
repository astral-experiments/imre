# CPython 3.14.5, Lib/encodings/utf_8.py.
import codecs


class StreamWriter(codecs.StreamWriter):
    encode = codecs.utf_8_encode


class IncrementalDecoder(codecs.BufferedIncrementalDecoder):
    _buffer_decode = codecs.utf_8_decode
