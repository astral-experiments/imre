class StreamReader(codecs.StreamReader):
    decode = codecs.utf_8_decode
