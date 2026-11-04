from _imre_base32 import decode as _decode32, encode as _encode32

def _b32encode(alphabet, s):
    if not isinstance(s, bytes_types):
        s = memoryview(s).tobytes()
    encoded = _encode32(s, alphabet)
    if encoded is not None:
        return encoded

    # Subclasses can override length, addition and slicing.
    leftover = len(s) % 5
    if leftover:
        s = s + b'\0' * (5 - leftover)
    encoded = bytearray()
    for i in range(0, len(s), 5):
        word = int.from_bytes(s[i:i + 5])
        for shift in range(35, -1, -5):
            encoded.append(alphabet[(word >> shift) & 31])
    if leftover:
        pad = (5 - leftover) * 8 // 5
        encoded[-pad:] = b'=' * pad
    return bytes(encoded)

def _b32decode(alphabet, s, casefold=False, map01=None):
    s = _bytes_from_decode_data(s)
    if len(s) % 8:
        raise binascii.Error('Incorrect padding')
    if map01 is not None:
        map01 = _bytes_from_decode_data(map01)
        assert len(map01) == 1, repr(map01)
        s = s.translate(bytes.maketrans(b'01', b'O' + map01))
    decoded = _decode32(s, alphabet, casefold)
    if decoded is not None:
        return decoded

    if casefold:
        s = s.upper()
    decoded = _decode32(s, alphabet, False)
    if decoded is not None:
        return decoded
    length = len(s)
    s = s.rstrip(b'=')
    padding = length - len(s)
    decoded = bytearray()
    for i in range(0, len(s), 8):
        word = 0
        for digit in s[i:i + 8]:
            value = alphabet.rfind(digit)
            if value < 0:
                raise binascii.Error('Non-base32 digit found')
            word = (word << 5) + value
        decoded += word.to_bytes(5)
    if length % 8 or padding not in (0, 1, 3, 4, 6):
        raise binascii.Error('Incorrect padding')
    if padding and decoded:
        last = (word << (5 * padding)).to_bytes(5)
        decoded[-5:] = last[:(43 - 5 * padding) // 8]
    return bytes(decoded)
