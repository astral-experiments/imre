"""Shift JIS codec backed by the native JIS X 0208 mapping."""
import codecs
from _codecs import _imre_codec_functions

_encode, _decode = _imre_codec_functions('shift_jis')


class Codec(codecs.Codec):
    encode = _encode

    def decode(self, input, errors='strict'):
        return _decode(input, errors)


class IncrementalEncoder(codecs.IncrementalEncoder):
    def encode(self, input, final=False):
        return _encode(input, self.errors)[0]


class IncrementalDecoder(codecs.BufferedIncrementalDecoder):
    def _buffer_decode(self, input, errors, final):
        return _decode(input, errors, final)


class StreamWriter(Codec, codecs.StreamWriter):
    pass
