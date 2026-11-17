import hashlib
from typing import Any

try:
    import _hashlib
except ImportError:
    _hashlib = None

results = [hashlib.sha256(), hashlib.new("shake-128")]
if _hashlib is not None:
    results.append(_hashlib.openssl_sha256())

def allocate(allocator: Any, args: Any) -> None:
    allocator(*args)

try:
    allocate(hashlib.new, ("missing", None))
except TypeError as exc:
    print(str(exc))

for result in results:
    cls = type(result)
    assert cls.__new__ is object.__new__
    for allocator, args in [(cls, ()), (cls.__new__, (cls,)), (object.__new__, (cls,))]:
        try:
            allocate(allocator, args)
        except TypeError as exc:
            assert str(exc) == f"cannot create '{cls.__module__}.{cls.__name__}' instances"
        else:
            raise AssertionError("allocated a hash result without its factory")

print("hash result allocation")
