"""Fixed-order UTF-16 and UTF-32 stream codecs, following CPython 3.14.5."""
import codecs

_codec = __name__.rsplit('.', 1)[1]
_name = _codec.replace('_', '-')
encode = getattr(codecs, _codec + '_encode')
_decode = getattr(codecs, _codec + '_decode')


def decode(input, errors='strict'):
    return getattr(codecs, _codec + '_decode')(input, errors, True)


class IncrementalEncoder(codecs.IncrementalEncoder):
    def encode(self, input, final=False):
        return getattr(codecs, _codec + '_encode')(input, self.errors)[0]


class IncrementalDecoder(codecs.BufferedIncrementalDecoder):
    _buffer_decode = _decode


class StreamWriter(codecs.StreamWriter):
    encode = encode


class StreamReader(codecs.StreamReader):
    decode = _decode


def getregentry():
    return codecs.CodecInfo(name=_name, encode=encode, decode=decode,
                            incrementalencoder=IncrementalEncoder,
                            incrementaldecoder=IncrementalDecoder,
                            streamreader=StreamReader, streamwriter=StreamWriter)
