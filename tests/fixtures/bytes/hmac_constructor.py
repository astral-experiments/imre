import hmac
from hmac import HMAC as constructor

# RFC 4231 section 4.3, Test Case 2 (HMAC-SHA-256).
key = b"Jefe"
message = b"what do ya want for nothing?"
expected = "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
assert hmac.new(key, message, digestmod="SHA-256").hexdigest() == expected
mac = constructor(key, digestmod="sha256")
mac.update(message[:12])
copy = mac.copy()
mac.update(message[12:])
copy.update(message[12:])
assert mac.hexdigest() == expected and copy.hexdigest() == expected
try:
    constructor(key, message, digestmod="missing")
except ValueError as exc:
    assert str(exc) == "unsupported hash type missing"
else:
    raise AssertionError("unknown HMAC digest name accepted")
