from hashlib import algorithms_available, algorithms_guaranteed, file_digest, md5, new
from io import BytesIO

assert "md5" in algorithms_available
assert "md5" in algorithms_guaranteed
assert md5(usedforsecurity=False).hexdigest() == "d41d8cd98f00b204e9800998ecf8427e"
assert md5(string=b"abc").hexdigest() == "900150983cd24fb0d6963f7d28e17f72"
assert md5(b"a" * 1000000).hexdigest() == "7707d6ae4e027c70eea2a935c2296f21"

for name in ["MD5", "ssl3-md5", "1.2.840.113549.2.5"]:
    h = new(name, data=b"ab", usedforsecurity=False)
    original = h.copy()
    h.update(b"c")
    assert original.digest() == md5(b"ab").digest()
    assert h.digest() == md5(b"abc").digest()
    print((h.name, h.block_size, h.digest_size, h.hexdigest(), h.hexdigest()))
for bad in ["MD-5", "RSA-MD5", "md5\x00"]:
    try:
        new(bad)
    except ValueError as exc:
        print(str(exc))

for size in [55, 56, 63, 64, 65]:
    stream = BytesIO(b"a" * size)
    assert file_digest(stream, md5).digest() == file_digest(stream, "md5").digest()
    print(md5(b"a" * size).hexdigest())
