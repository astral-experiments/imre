# CPython 3.14.5, Lib/encodings/ascii.py.
import codecs


class Codec(codecs.Codec):

    # Note: Binding these as C functions will result in the class not
    # converting them to methods. This is intended.
    encode = codecs.ascii_encode
    decode = codecs.ascii_decode


class StreamWriter(Codec,codecs.StreamWriter):
    pass
