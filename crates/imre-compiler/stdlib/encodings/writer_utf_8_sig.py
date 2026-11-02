# CPython 3.14.5, Lib/encodings/utf_8_sig.py.
import codecs


def encode(input, errors='strict'):
    return (codecs.BOM_UTF8 + codecs.utf_8_encode(input, errors)[0],
            len(input))


class StreamWriter(codecs.StreamWriter):
    def reset(self):
        codecs.StreamWriter.reset(self)
        try:
            del self.encode
        except AttributeError:
            pass

    def encode(self, input, errors='strict'):
        self.encode = codecs.utf_8_encode
        return encode(input, errors)
