from hashlib import sha256


def flag() -> bool:
    print("flag")
    return False


def data() -> bytes:
    print("data")
    return b"abc"


h = sha256(usedforsecurity=flag(), data=data())
alias = h
clone = h.copy()
before = h.digest()
print(alias is h)
print(clone is not h)
print(h == alias)
print(h != clone)
print(h.name)
print(h.digest_size)
print(h.block_size)
print(sha256(data()).digest_size)
print(alias.update(b"def") is None)
print(before == clone.digest())
print(h.digest() == alias.digest())
print(h.digest() != before)
print(h.hexdigest())
print(h.hexdigest())
clone.update(b"other")
print(clone.hexdigest())
print(h.hexdigest())

handles = [h, clone]
handles[0].update(b"!")
print(alias.digest())
print(handles[1].digest())
