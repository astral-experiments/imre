import hashlib

h = hashlib.sha256(b"a")
alias = h
branch = h.copy()
cls = type(h)
assert cls is type(branch)
assert cls is type(hashlib.new(h.name))
assert h.__class__ is cls
assert alias is h
assert branch is not h
assert repr(alias) == str(h)
assert repr(branch) != repr(h)
assert h.name in repr(h)
assert cls.__module__ + "." + cls.__name__ in repr(h)
assert f"0x{id(h):x}" in repr(h)
assert id(branch) != id(h)
before = repr(h)
h.update(b"b")
assert before == repr(alias)
assert alias.digest() != branch.digest()
print("hash identity")
