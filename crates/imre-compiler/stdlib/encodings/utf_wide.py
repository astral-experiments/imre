"""BOM-aware UTF-16 and UTF-32 stream codecs, following CPython 3.14.5."""
import codecs
import sys


if __name__ == 'encodings.utf_16':
    _name = 'utf-16'
    _width = 2
    encode = codecs.utf_16_encode
    _decode = codecs.utf_16_ex_decode
else:
    _name = 'utf-32'
    _width = 4
    encode = codecs.utf_32_encode
    _decode = codecs.utf_32_ex_decode

_native_order = -1 if sys.byteorder == 'little' else 1


def decode(input, errors='strict'):
    return _decode(input, errors, 0, True)[:2]


class IncrementalEncoder(codecs.IncrementalEncoder):
    def __init__(self, errors='strict'):
        super().__init__(errors)
        self.order = 0

    def encode(self, input, final=False):
        data, _ = encode(input, self.errors, self.order)
        self.order = _native_order
        return data

    def reset(self):
        self.order = 0

    def getstate(self):
        return 2 if self.order == 0 else 0

    def setstate(self, state):
        self.order = 0 if state else _native_order


class IncrementalDecoder(codecs.BufferedIncrementalDecoder):
    def __init__(self, errors='strict'):
        super().__init__(errors)
        self.order = 0

    def _buffer_decode(self, input, errors, final):
        text, consumed, order = _decode(input, errors, self.order, final)
        if order == 0 and consumed >= _width:
            raise UnicodeDecodeError(_name, input, 0, _width, 'Stream does not start with BOM')
        self.order = order
        return text, consumed

    def reset(self):
        super().reset()
        self.order = 0

    def getstate(self):
        buffer, _ = super().getstate()
        return buffer, 2 if self.order == 0 else int(self.order != _native_order)

    def setstate(self, state):
        super().setstate(state)
        order = state[1]
        if order == 0:
            self.order = _native_order
        elif order == 1:
            self.order = -_native_order
        else:
            self.order = 0


class StreamWriter(codecs.StreamWriter):
    def __init__(self, stream, errors='strict'):
        super().__init__(stream, errors)
        self.order = 0

    def reset(self):
        super().reset()
        self.order = 0

    def encode(self, input, errors='strict'):
        result = encode(input, errors, self.order)
        self.order = _native_order
        return result


def getregentry():
    return codecs.CodecInfo(name=_name, encode=encode, decode=decode,
                            incrementalencoder=IncrementalEncoder,
                            incrementaldecoder=IncrementalDecoder,
                            streamwriter=StreamWriter)
