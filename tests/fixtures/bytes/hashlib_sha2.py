from hashlib import algorithms_available, algorithms_guaranteed, new, sha224, sha384, sha512

assert sha224(b"abc").hexdigest() == "23097d223405d8228642a477bda255b32aadbce4bda0b3f7e36c9da7"
assert sha384(b"abc").hexdigest() == (
    "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed"
    "8086072ba1e7cc2358baeca134c825a7"
)
assert sha512(data=b"abc", usedforsecurity=False).hexdigest() == (
    "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a"
    "2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
)

for size in [0, 55, 56, 63, 64, 111, 112, 127, 128, 129, 1000000]:
    data = b"a" * size
    for h in [sha224(data), sha384(data), sha512(data)]:
        assert h.name in algorithms_available
        assert h.name in algorithms_guaranteed
        duplicate = h.copy()
        duplicate.update(b"!")
        print((h.name, h.block_size, h.digest_size, h.hexdigest(), duplicate.hexdigest()))
        assert duplicate.digest() == new(h.name, data + b"!").digest()

for name in ["SHA-224", "sha2-224", "2.16.840.1.101.3.4.2.4",
             "SHA-384", "sha2-384", "2.16.840.1.101.3.4.2.2",
             "SHA-512", "sha2-512", "2.16.840.1.101.3.4.2.3"]:
    print(new(name).name)
