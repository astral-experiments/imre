import zlib

encoder = zlib.compressobj()
packed = encoder.compress(b"") + encoder.compress(b"a")
try:
    encoder.compress("b")
except TypeError:
    pass
else:
    raise AssertionError("compress accepted text")
packed += encoder.compress(b"b") + encoder.compress(b"c") + encoder.flush()
print(zlib.decompress(packed))
