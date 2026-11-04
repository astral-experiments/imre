"""IDNA (RFC 3490) with CPython 3.14.5 label buffering and stream interfaces."""

import codecs
import re
from _codecs import _imre_codec_functions, _imre_idna_label

dots = re.compile("[\u002E\u3002\uFF0E\uFF61]")
ace_prefix = b"xn--"
_encode, _decode = _imre_codec_functions("idna")


def nameprep(label):
    return _imre_idna_label("nameprep", label)


def ToASCII(label):
    return _imre_idna_label("ToASCII", label)


def ToUnicode(label):
    if isinstance(label, str):
        if len(label) > 1024:
            data = label.encode("utf-8", errors="backslashreplace")
            raise UnicodeDecodeError("idna", data, 0, len(data), "label way too long")
        try:
            label = label.encode("ascii")
        except UnicodeEncodeError:
            label = nameprep(label)
            try:
                label = label.encode("ascii")
            except UnicodeEncodeError as exc:
                raise UnicodeEncodeError(
                    "idna", label, exc.start, exc.end, "Invalid character in IDN label"
                )
    return _imre_idna_label("ToUnicode", label)


class Codec(codecs.Codec):
    def encode(self, input, errors='strict'):

        if errors != 'strict':
            # IDNA is quite clear that implementations must be strict
            raise UnicodeError(f"Unsupported error handling: {errors}")

        if not input:
            return b'', 0

        # The shared encoder retains the original code points, including surrogates.
        if type(input) is str:
            return _encode(input)

        # Preserve the standard module's method dispatch for custom inputs.
        try:
            result = input.encode('ascii')
        except UnicodeEncodeError:
            pass
        else:
            # ASCII name: fast path
            labels = result.split(b'.')
            for i, label in enumerate(labels[:-1]):
                if len(label) == 0:
                    offset = sum(len(l) for l in labels[:i]) + i
                    raise UnicodeEncodeError("idna", input, offset, offset+1,
                                             "label empty")
            for i, label in enumerate(labels):
                if len(label) >= 64:
                    offset = sum(len(l) for l in labels[:i]) + i
                    raise UnicodeEncodeError("idna", input, offset, offset+len(label),
                                             "label too long")
            return result, len(input)

        result = bytearray()
        if isinstance(input, str):
            labels = _imre_idna_label("split", input)
        else:
            labels = dots.split(input)
        if labels and not labels[-1]:
            trailing_dot = b'.'
            del labels[-1]
        else:
            trailing_dot = b''
        for i, label in enumerate(labels):
            if result:
                # Join with U+002E
                result.extend(b'.')
            try:
                result.extend(ToASCII(label))
            except (UnicodeEncodeError, UnicodeDecodeError) as exc:
                offset = sum(len(l) for l in labels[:i]) + i
                raise UnicodeEncodeError(
                    "idna",
                    input,
                    offset + exc.start,
                    offset + exc.end,
                    exc.reason,
                )
        return bytes(result+trailing_dot), len(input)

    def decode(self, input, errors='strict'):

        if errors != 'strict':
            raise UnicodeError(f"Unsupported error handling: {errors}")

        if not input:
            return "", 0

        # IDNA allows decoding to operate on Unicode strings, too.
        if not isinstance(input, bytes):
            # XXX obviously wrong, see #3232
            input = bytes(input)

        if type(input) is bytes:
            return _decode(input)

        if ace_prefix not in input.lower():
            # Fast path
            try:
                return input.decode('ascii'), len(input)
            except UnicodeDecodeError:
                pass

        labels = input.split(b".")

        if labels and len(labels[-1]) == 0:
            trailing_dot = '.'
            del labels[-1]
        else:
            trailing_dot = ''

        result = []
        for i, label in enumerate(labels):
            try:
                u_label = ToUnicode(label)
            except (UnicodeEncodeError, UnicodeDecodeError) as exc:
                offset = sum(len(x) for x in labels[:i]) + len(labels[:i])
                raise UnicodeDecodeError(
                    "idna", input, offset+exc.start, offset+exc.end, exc.reason)
            else:
                result.append(u_label)

        return ".".join(result)+trailing_dot, len(input)

class IncrementalEncoder(codecs.BufferedIncrementalEncoder):
    def _buffer_encode(self, input, errors, final):
        if errors != 'strict':
            # IDNA is quite clear that implementations must be strict
            raise UnicodeError(f"Unsupported error handling: {errors}")

        if not input:
            return (b'', 0)

        labels = _imre_idna_label("split", input)
        trailing_dot = b''
        if labels:
            if not labels[-1]:
                trailing_dot = b'.'
                del labels[-1]
            elif not final:
                # Keep potentially unfinished label until the next call
                del labels[-1]
                if labels:
                    trailing_dot = b'.'

        result = bytearray()
        size = 0
        for label in labels:
            if size:
                # Join with U+002E
                result.extend(b'.')
                size += 1
            try:
                result.extend(ToASCII(label))
            except (UnicodeEncodeError, UnicodeDecodeError) as exc:
                raise UnicodeEncodeError(
                    "idna",
                    input,
                    size + exc.start,
                    size + exc.end,
                    exc.reason,
                )
            size += len(label)

        result += trailing_dot
        size += len(trailing_dot)
        return (bytes(result), size)

class IncrementalDecoder(codecs.BufferedIncrementalDecoder):
    def _buffer_decode(self, input, errors, final):
        if errors != 'strict':
            raise UnicodeError(f"Unsupported error handling: {errors}")

        if not input:
            return ("", 0)

        # IDNA allows decoding to operate on Unicode strings, too.
        if isinstance(input, str):
            labels = _imre_idna_label("split", input)
        else:
            # Must be ASCII string
            try:
                input = str(input, "ascii")
            except (UnicodeEncodeError, UnicodeDecodeError) as exc:
                raise UnicodeDecodeError("idna", input,
                                         exc.start, exc.end, exc.reason)
            labels = input.split(".")

        trailing_dot = ''
        if labels:
            if not labels[-1]:
                trailing_dot = '.'
                del labels[-1]
            elif not final:
                # Keep potentially unfinished label until the next call
                del labels[-1]
                if labels:
                    trailing_dot = '.'

        result = []
        size = 0
        for label in labels:
            try:
                u_label = ToUnicode(label)
            except (UnicodeEncodeError, UnicodeDecodeError) as exc:
                raise UnicodeDecodeError(
                    "idna",
                    input.encode("ascii", errors="backslashreplace"),
                    size + exc.start,
                    size + exc.end,
                    exc.reason,
                )
            else:
                result.append(u_label)
            if size:
                size += 1
            size += len(label)

        result = ".".join(result) + trailing_dot
        size += len(trailing_dot)
        return (result, size)

class StreamWriter(Codec,codecs.StreamWriter):
    pass

class StreamReader(Codec,codecs.StreamReader):
    pass

### encodings module API

def getregentry():
    return codecs.CodecInfo(
        name='idna',
        encode=Codec().encode,
        decode=Codec().decode,
        incrementalencoder=IncrementalEncoder,
        incrementaldecoder=IncrementalDecoder,
        streamwriter=StreamWriter,
        streamreader=StreamReader,
    )
