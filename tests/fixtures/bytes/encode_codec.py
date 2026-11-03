print("text".encode("ascii"))

assert ''.encode('UTF-16-LE') == b''
assert '\ufeffA🙂'.encode('UTF_16LE') == b'\xff\xfeA\x00=\xd8B\xde'
assert '\ufeffA🙂'.encode('unicodebigunmarked') == b'\xfe\xff\x00A\xd8=\xdeB'

assert ''.encode('UTF-32-LE') == b''
assert '\ufeffA🙂'.encode('UTF_32LE') == b'\xff\xfe\x00\x00A\x00\x00\x00B\xf6\x01\x00'
assert '\ufeffA🙂'.encode('utf-32be') == b'\x00\x00\xfe\xff\x00\x00\x00A\x00\x01\xf6B'

# Native-order stateless encoders include a BOM even for an empty string.
bom = ''.encode('utf-16')
if bom == b'\xff\xfe':
    assert '😀'.encode('utf-32') == b'\xff\xfe\x00\x00\x00\xf6\x01\x00'
else:
    assert bom == b'\xfe\xff'
    assert '😀'.encode('utf-32') == b'\x00\x00\xfe\xff\x00\x01\xf6\x00'

assert '表ｱ〜'.encode('shift_jis') == b'\x95\\\xb1\x81\x60'
assert '¥‾'.encode('sjis') == b'\\~'
