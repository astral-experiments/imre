import sys

sys.modules["_hashlib"] = None
sys.modules["_hmac"] = None
hmac = __import__("hmac")
hashlib = __import__("hashlib")
md5 = __import__("_md5").md5
events = []
message = b"what do ya want for nothing?"
expected = bytes.fromhex("750c783e6ab0b503eaa86e310a5db738")


class Key(bytes):
    def __len__(self):
        events.append("len")
        return 4

    def __getattribute__(self, name):
        if name == "ljust":
            events.append("get ljust")
        return super().__getattribute__(name)

    def __add__(self, other):
        raise AssertionError("HMAC used subclass addition for padding")


def new(name, data=b""):
    events.append((name, data))
    return md5(data)


saved = hashlib.new
try:
    hashlib.new = new
    assert hmac.digest(Key(b"Jefe"), message, "foo") == expected
    assert events == [("foo", b""), ("foo", b""), "len", "get ljust"]
    events.clear()
    try:
        hmac.digest(memoryview(b"Jefe"), message, "sha256")
    except AttributeError as exc:
        assert "ljust" in str(exc)
    else:
        raise AssertionError("short view key skipped the Python padding protocol")
    assert events == [("sha256", b""), ("sha256", b"")]
finally:
    hashlib.new = saved
