import codecs
from _codecs import _imre_codec_functions

# Each encoding module uses its own native character map and Codec class.
_encode, _decode = _imre_codec_functions(__name__.rsplit('.', 1)[1])


class Codec(codecs.Codec):
    def encode(self, input, errors='strict'):
        return _encode(input, errors)

    def decode(self, input, errors='strict'):
        return _decode(input, errors)


class StreamWriter(Codec, codecs.StreamWriter):
    pass


class IncrementalDecoder(codecs.IncrementalDecoder):
    def decode(self, input, final=False):
        return _decode(input, self.errors)[0]
