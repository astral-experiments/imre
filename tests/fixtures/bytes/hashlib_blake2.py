from hashlib import algorithms_available, algorithms_guaranteed, blake2b, blake2s, new

# CPython 3.14.5 Lib/test/test_hashlib.py, test_case_blake2{b,s}_1.
assert blake2b(b"abc").hexdigest() == (
    "ba80a53f981c4d0d6a2797b69f12f6e94c212f14685ac4b74b12bb6fdbffa2d1"
    "7d87c5392aab792dc252d5de4533cc9518d38aa8dbf1925ab92386edd4009923"
)
assert blake2s(b"abc").hexdigest() == "508c5e8c327c14e2e1a72ba34eeb452f37458b209ed63a294d999b4c86675982"

# The full message, parameters and digests are from CPython's
# test_case_blake2b_all_parameters and test_case_blake2s_all_parameters.
params = dict(digest_size=16, key=bytearray(b"bar"), salt=b"baz", person=b"bing",
              fanout=2, depth=3, leaf_size=4, node_offset=5, node_depth=6,
              inner_size=7, last_node=True)
for cls, name, block, max_offset, expected in (
    (blake2b, "blake2b", 128, (1 << 64) - 1, "920568b0c5873b2f0ab67bedb6cf1b2b"),
    (blake2s, "blake2s", 64, (1 << 48) - 1, "bf2a8f7fe3c555012a6f8046e646bc75"),
):
    assert name in algorithms_guaranteed and name in algorithms_available
    full = cls(b"foo", **params)
    assert full.name == name and full.block_size == block and full.digest_size == 16
    assert full.hexdigest() == expected

    prefix = new(name=name, string=b"f", **params)
    branch = prefix.copy()
    prefix.update(bytearray(b"oo"))
    assert prefix.hexdigest() == expected and prefix.digest() == full.digest()
    branch.update(b"ood")
    assert branch.digest() == cls(b"food", **params).digest()
    assert branch.digest() != prefix.digest()

    # The maximum node offset differs between the two variants.
    edge = cls(b"foo", leaf_size=(1 << 32) - 1, node_offset=max_offset)
    assert edge.digest() == new(name, b"foo", leaf_size=(1 << 32) - 1, node_offset=max_offset).digest()

try:
    new("blake2s", node_offset=1 << 48)
except OverflowError as exc:
    assert str(exc) == "node_offset is too large"
else:
    raise AssertionError("BLAKE2s accepted a 49-bit node offset")
