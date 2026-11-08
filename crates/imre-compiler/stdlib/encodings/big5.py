"""Big5 codec backed by the native CPython Taiwan mapping."""
import codecs
import sys
from _codecs import _imre_codec_functions

_encode, _decode = _imre_codec_functions("big5")


class Codec(codecs.Codec):
    encode = _encode

    @staticmethod
    def decode(input, errors="strict"):
        return _decode(input, errors)


class _ErrorMode:
    _errors = None

    @property
    def errors(self):
        return self._errors

    @errors.setter
    def errors(self, value):
        if not isinstance(value, str):
            raise TypeError("errors must be a string")
        if "\0" in value:
            if self._errors is None:
                raise ValueError("embedded null character")
            value = value.partition("\0")[0]
        self._errors = value


class IncrementalEncoder(_ErrorMode, codecs.IncrementalEncoder):
    _state = 0

    def encode(self, input, final=False):
        bool(final)
        if not self.buffer:
            return _encode(input, self.errors)[0]
        text = input if isinstance(input, str) else str(input)
        previous = self.buffer
        self.buffer = ""
        try:
            return _encode(previous + text, self.errors)[0]
        except BaseException:
            self.buffer = previous
            raise

    def getstate(self):
        pending = self.buffer.encode()
        data = bytes([len(pending)]) + pending + self._state.to_bytes(8, "little")
        return int.from_bytes(data, "little")

    def setstate(self, state, /):
        if not isinstance(state, int):
            name = "None" if state is None else type(state).__name__
            raise TypeError("setstate() argument must be int, not " + name)
        # CPython serializes the state as a length, up to eight UTF-8 bytes
        # of pending text, and eight bytes of codec state in little-endian order.
        data = state.to_bytes(17, "little")
        size = data[0]
        if size > 8:
            raise UnicodeError("pending buffer too large")
        self.buffer = data[1:1 + size].decode("utf-8")
        self._state = int.from_bytes(data[1 + size:9 + size], "little")

    def reset(self):
        self.buffer = ""


def _pending(buffer, data, consumed):
    if consumed == len(data):
        return buffer
    if len(buffer) + len(data) - consumed > 8:
        raise UnicodeDecodeError("big5", bytes(data), 0, len(data), "pending buffer overflow")
    return buffer + bytes(data[consumed:])


class IncrementalDecoder(_ErrorMode, codecs.BufferedIncrementalDecoder):
    _state = 0

    def decode(self, input, final=False):
        final = bool(final)
        if isinstance(input, bytes):
            return self._decode_chunk(input, final)
        # Keep the caller's export alive across error callbacks, including when
        # joining a pending lead byte to an incoming mutable buffer.
        with memoryview(input) as view:
            if not view.c_contiguous:
                raise BufferError("memoryview: underlying buffer is not C-contiguous")
            with view.cast("B") as octets:
                return self._decode_chunk(octets, final)

    def _decode_chunk(self, chunk, final):
        previous = self.buffer
        data = previous + bytes(chunk) if previous else chunk
        self.buffer = b""
        if previous and final:
            # CPython restores pending input only if the final truncation handler
            # raises. An illegal pair in the same input consumes the old buffer.
            incomplete = [False]
            try:
                output, consumed = _decode(data, self.errors, final, incomplete)
            except BaseException:
                if incomplete[0]:
                    self.buffer = previous
                raise
        else:
            output, consumed = _decode(data, self.errors, final)
        self.buffer = _pending(self.buffer, data, consumed)
        return output

    def getstate(self):
        return self.buffer, self._state

    def setstate(self, state, /):
        if not isinstance(state, tuple) or len(state) != 2:
            raise TypeError("setstate(): illegal state argument")
        buffer, flags = state
        if not isinstance(buffer, bytes) or not isinstance(flags, int):
            raise TypeError("setstate(): illegal state argument")
        if flags < 0:
            raise OverflowError("can't convert negative int to unsigned")
        if flags.bit_length() > 64:
            raise OverflowError("int too big to convert")
        if len(buffer) > 8:
            raise UnicodeDecodeError("big5", buffer, 0, len(buffer), "pending buffer too large")
        self.buffer = buffer
        self._state = flags


class StreamReader(Codec, _ErrorMode, codecs.StreamReader):
    def _read(self, method, size):
        if size is None:
            size = -1
        if not isinstance(size, int):
            raise TypeError("arg 1 must be an integer")
        if size > sys.maxsize or size < -sys.maxsize - 1:
            raise OverflowError("Python int too large to convert to C ssize_t")
        if size == 0:
            return ""
        while True:
            read = getattr(self.stream, method)
            chunk = read() if size < 0 else read(size)
            if not isinstance(chunk, bytes):
                raise TypeError("stream function returned a non-bytes object (%s)" % type(chunk).__name__)
            data = self.bytebuffer + chunk
            self.bytebuffer = b""
            output, consumed = _decode(data, self.errors, not chunk or size < 0)
            self.bytebuffer = _pending(self.bytebuffer, data, consumed)
            if output or size < 0 or not data:
                return output
            size = 1

    def read(self, size=None, /):
        return self._read("read", size)

    def readline(self, size=None, /):
        return self._read("readline", size)

    def readlines(self, sizehint=None, /):
        return self._read("read", sizehint).splitlines(True)


class StreamWriter(Codec, _ErrorMode, codecs.StreamWriter):
    def writelines(self, lines, /):
        if isinstance(lines, dict) or not hasattr(type(lines), "__getitem__"):
            raise TypeError("arg must be a sequence object")
        i = 0
        while i < len(lines):
            self.write(lines[i])
            i += 1


def getregentry():
    return codecs.CodecInfo(
        name="big5",
        encode=Codec.encode,
        decode=Codec.decode,
        incrementalencoder=IncrementalEncoder,
        incrementaldecoder=IncrementalDecoder,
        streamreader=StreamReader,
        streamwriter=StreamWriter,
    )
