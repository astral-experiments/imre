# CPython 3.14.5, Lib/encodings/latin_1.py.
import codecs


class Codec(codecs.Codec):

    # Note: Binding these as C functions will result in the class not
    # converting them to methods. This is intended.
    encode = codecs.latin_1_encode
    decode = codecs.latin_1_decode


class StreamWriter(Codec,codecs.StreamWriter):
    pass
