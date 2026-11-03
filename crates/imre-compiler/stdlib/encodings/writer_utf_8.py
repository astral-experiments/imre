# CPython 3.14.5, Lib/encodings/utf_8.py.
import codecs


class StreamWriter(codecs.StreamWriter):
    encode = codecs.utf_8_encode


class IncrementalEncoder(codecs.IncrementalEncoder):
    def encode(self, input, final=False):
        return codecs.utf_8_encode(input, self.errors)[0]


class IncrementalDecoder(codecs.BufferedIncrementalDecoder):
    _buffer_decode = codecs.utf_8_decode
