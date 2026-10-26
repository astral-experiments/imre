from hashlib import algorithms_available, new, sha1, sha256

print("sha1" in algorithms_available)
print("sha256" in algorithms_available)

for data in [b"", b"abc", b"\x00\xff", b"x" * 55, b"x" * 56, b"x" * 64, b"x" * 65]:
    direct = sha1(data, usedforsecurity=False)
    print((direct.name, direct.block_size, direct.digest_size, direct.hexdigest()))
    print((len(direct.digest()), sha1(data).hexdigest() == direct.hexdigest()))

h = sha1(b"a")
alias = h
clone = h.copy()
print((h is alias, h is not clone))
print(h.update(b"b") is None)
clone.update(b"x")
print((alias.digest() == sha1(b"ab").digest(), clone.digest() == sha1(b"ax").digest()))
print((h.hexdigest(), h.hexdigest()))

for name in ["sha1", "SHA-1", "ssl3-sha1", "1.3.14.3.2.26", "sha256", "SHA2-256"]:
    digest = new(name, b"abc")
    print((digest.name, digest.digest_size, digest.block_size, digest.digest() == digest.copy().digest()))

both = [sha1(b"abc"), sha256(b"abc")]
for handle in both:
    print((handle.name, handle.digest_size, handle.hexdigest()))

for name in ["", "SHA", "sha_1", "RSA-SHA1", "sha1\x00"]:
    try:
        new(name)
    except ValueError as exc:
        print(repr(str(exc)))
