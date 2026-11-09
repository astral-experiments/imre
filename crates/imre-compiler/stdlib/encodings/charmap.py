"""Single-byte codec backed by the registered native character map."""
import codecs
from _codecs import _imre_codec_functions

# Each encoding module uses its own native character map and Codec class.
_name = __name__.rsplit('.', 1)[1].replace('_', '-')
_encode, _decode = _imre_codec_functions(_name)


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


class StreamReader(Codec, codecs.StreamReader):
    pass


class IncrementalEncoder(codecs.IncrementalEncoder):
    def encode(self, input, final=False):
        return _encode(input, self.errors)[0]


def getregentry():
    return codecs.CodecInfo(
        name=_name,
        encode=Codec().encode,
        decode=Codec().decode,
        incrementalencoder=IncrementalEncoder,
        incrementaldecoder=IncrementalDecoder,
        streamreader=StreamReader,
        streamwriter=StreamWriter,
    )
