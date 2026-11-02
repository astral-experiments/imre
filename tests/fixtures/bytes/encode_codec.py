print("text".encode("ascii"))

assert ''.encode('UTF-16-LE') == b''
assert '\ufeffA🙂'.encode('UTF_16LE') == b'\xff\xfeA\x00=\xd8B\xde'
assert '\ufeffA🙂'.encode('unicodebigunmarked') == b'\xfe\xff\x00A\xd8=\xdeB'

assert ''.encode('UTF-32-LE') == b''
assert '\ufeffA🙂'.encode('UTF_32LE') == b'\xff\xfe\x00\x00A\x00\x00\x00B\xf6\x01\x00'
assert '\ufeffA🙂'.encode('utf-32be') == b'\x00\x00\xfe\xff\x00\x00\x00A\x00\x01\xf6B'
