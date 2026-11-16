from hashlib import blake2b, blake2s, new


class Index:
    def __init__(self, value: int) -> None:
        self.value = value

    def __index__(self) -> int:
        return self.value


size = Index(16)
assert blake2b(b"abc", digest_size=size).hexdigest() == "cf4ab791c62b8d2b2109c90275287816"
assert blake2s(b"abc", digest_size=size).hexdigest() == "aa4938119b1dc7b87cbad0ffd200d0ae"
tree = new(
    "blake2b", b"abc", digest_size=size, fanout=Index(2), depth=Index(3),
    leaf_size=Index(4), node_offset=Index(5), node_depth=Index(6), inner_size=Index(7),
)
assert tree.hexdigest() == "e94db69296b064da900fa69841d4da66"
