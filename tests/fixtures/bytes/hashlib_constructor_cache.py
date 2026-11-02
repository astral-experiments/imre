import hashlib
import sys
from hashlib import __builtin_constructor_cache as cache
from hashlib import __get_builtin_constructor as lookup

assert cache is hashlib.__builtin_constructor_cache
cache.clear()
sys.modules["_sha2"] = None
try:
    lookup("SHA256")
except ValueError as exc:
    assert exc.args == ("unsupported hash type SHA256",)
else:
    raise AssertionError("blocked provider accepted")
assert cache == {}

# Lookup uses the current module and caches both spellings of each constructor.
sys.modules["_sha2"] = hashlib
assert lookup("sha256") is hashlib.sha256
assert lookup("SHA224") is hashlib.sha224
assert set(cache) == {"SHA224", "sha224", "SHA256", "sha256"}
sys.modules["_sha2"] = None
assert lookup("SHA256") is hashlib.sha256

cache["sha256"] = False
assert lookup("sha256") is False
