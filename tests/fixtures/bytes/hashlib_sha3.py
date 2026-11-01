from hashlib import algorithms_available, algorithms_guaranteed, new, sha3_224, sha3_256, sha3_384, sha3_512


# Empty-message vectors from FIPS 202, also exercised by CPython's test_hashlib.
assert sha3_224().hexdigest() == "6b4e03423667dbb73b6e15454f0eb1abd4597f9a1b078e3f5b5a6bc7"
assert sha3_256().hexdigest() == "a7ffc6f8bf1ed76651c14756a061d662f580ff4de43b49fa82d80a4b80f8434a"
assert sha3_384().hexdigest() == (
    "0c63a75b845e4f7d01107d852e4c2485c51a50aaaa94fc61995e71bbee983a2a"
    "c3713831264adb47fb6bd1e058d5f004"
)
assert sha3_512().hexdigest() == (
    "a69f73cca23a9ac5c8b567dc185a756e97c982164fe25859e0d1dcc1475c80a6"
    "15b2123af1f5f94c11e3e9402c3ac558f500199d95b6d3e301758586281dcd26"
)

for h in [sha3_224(), sha3_256(), sha3_384(), sha3_512()]:
    assert h.name in algorithms_available
    assert h.name in algorithms_guaranteed
    data = b"x" * (h.block_size - 1)
    h.update(data)
    branch = h.copy()
    branch.update(b"yz")
    assert h.digest() == new(h.name, data).digest()
    assert branch.digest() == new(h.name, data + b"yz").digest()
    print((h.name, h.block_size, h.digest_size, h.hexdigest(), branch.hexdigest()))

for name in ["SHA3-224", "SHA3-256", "Sha3-384", "2.16.840.1.101.3.4.2.10"]:
    print(new(name, string=b"abc", usedforsecurity=False).name)
try:
    new("SHA3_256")
except ValueError as exc:
    print(str(exc))
