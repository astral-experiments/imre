class StreamReader(Codec, codecs.StreamReader):
    pass


class IncrementalEncoder(codecs.IncrementalEncoder):
    def encode(self, input, final=False):
        return _encode(input, self.errors)[0]
