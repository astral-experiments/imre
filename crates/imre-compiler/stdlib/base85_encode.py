from _imre_base85 import encode as _encode85

# Index the alphabet directly instead of building a 7,225-entry table on first use.
_a85alphabet = bytes(range(33, 118))
_A85START = b"<~"
_A85END = b"~>"

def _85encode(b, alphabet, pad=False, foldnuls=False, foldspaces=False):
    # Helper function for a85encode and b85encode
    if not isinstance(b, (bytes, bytearray)):
        b = memoryview(b).tobytes()

    encoded = _encode85(b, alphabet, pad, foldnuls, foldspaces)
    if encoded is not None:
        return encoded

    padding = (-len(b)) % 4
    if padding:
        b = b + b'\0' * padding
    words = struct.Struct('!%dI' % (len(b) // 4)).unpack(b)

    chunks = [b'z' if foldnuls and not word else
              b'y' if foldspaces and word == 0x20202020 else
              bytes((alphabet[word // 85**4],
                     alphabet[word // 85**3 % 85],
                     alphabet[word // 85**2 % 85],
                     alphabet[word // 85 % 85],
                     alphabet[word % 85]))
              for word in words]

    if padding and not pad:
        if chunks[-1] == b'z':
            chunks[-1] = alphabet[:1] * 5
        chunks[-1] = chunks[-1][:-padding]

    return b''.join(chunks)

def a85encode(b, *, foldspaces=False, wrapcol=0, pad=False, adobe=False):
    """Encode bytes-like object b using Ascii85 and return a bytes object.

    foldspaces is an optional flag that uses the special short sequence 'y'
    instead of 4 consecutive spaces (ASCII 0x20) as supported by 'btoa'. This
    feature is not supported by the "standard" Adobe encoding.

    wrapcol controls whether the output should have newline (b'\\n') characters
    added to it. If this is non-zero, each output line will be at most this
    many characters long, excluding the trailing newline.

    pad controls whether the input is padded to a multiple of 4 before
    encoding. Note that the btoa implementation always pads.

    adobe controls whether the encoded byte sequence is framed with <~ and ~>,
    which is used by the Adobe implementation.
    """
    result = _85encode(b, _a85alphabet, pad, True, foldspaces)

    if adobe:
        result = _A85START + result
    if wrapcol:
        wrapcol = max(2 if adobe else 1, wrapcol)
        chunks = [result[i: i + wrapcol]
                  for i in range(0, len(result), wrapcol)]
        if adobe:
            if len(chunks[-1]) + 2 > wrapcol:
                chunks.append(b'')
        result = b'\n'.join(chunks)
    if adobe:
        result += _A85END

    return result

# The following code is originally taken (with permission) from Mercurial

_b85alphabet = (b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"
                b"abcdefghijklmnopqrstuvwxyz!#$%&()*+-;<=>?@^_`{|}~")
_b85dec = None

def b85encode(b, pad=False):
    """Encode bytes-like object b in base85 format and return a bytes object.

    If pad is true, the input is padded with b'\\0' so its length is a multiple of
    4 bytes before encoding.
    """
    return _85encode(b, _b85alphabet, pad)
