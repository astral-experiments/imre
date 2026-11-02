print("text".encode("ascii"))

assert ''.encode('UTF-16-LE') == b''
assert '\ufeffA🙂'.encode('UTF_16LE') == b'\xff\xfeA\x00=\xd8B\xde'
assert '\ufeffA🙂'.encode('unicodebigunmarked') == b'\xfe\xff\x00A\xd8=\xdeB'
