class StreamReader(codecs.StreamReader):
    def reset(self):
        codecs.StreamReader.reset(self)
        try:
            del self.decode
        except AttributeError:
            pass

    def decode(self, input, errors='strict'):
        if len(input) < 3:
            if codecs.BOM_UTF8.startswith(input):
                # Wait for the rest of a possible BOM.
                return ("", 0)
        elif input[:3] == codecs.BOM_UTF8:
            self.decode = codecs.utf_8_decode
            (output, consumed) = codecs.utf_8_decode(input[3:], errors)
            return (output, consumed + 3)
        self.decode = codecs.utf_8_decode
        return codecs.utf_8_decode(input, errors)
