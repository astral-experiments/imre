"""The HMAC-SHA1, URL-safe timestamp signer used by Flask's default cookie profile."""

import base64
import hashlib
import hmac


def hmac_sha1(key: bytes, message: bytes) -> bytes:
    if len(key) > 64:
        key = hashlib.sha1(key).digest()
    key = key + bytes(64 - len(key))
    inner: list[int] = []
    outer: list[int] = []
    for byte in key:
        inner.append(byte ^ 54)
        outer.append(byte ^ 92)
    return hashlib.sha1(bytes(outer) + hashlib.sha1(bytes(inner) + message).digest()).digest()


def encode_url(data: bytes) -> bytes:
    encoded = base64.urlsafe_b64encode(data)
    end = len(encoded)
    while end and encoded[end - 1] == 61:
        end -= 1
    return encoded[:end]


def decode_url(data: bytes) -> bytes:
    return base64.b64decode(data + b"=" * (-len(data) % 4), b"-_", validate=True)


class FrozenSigner:
    key: bytes
    max_age: int

    def __init__(self, secret: bytes, salt: bytes, max_age: int) -> None:
        assert secret, "a signing key is required"
        self.key = hmac_sha1(secret, salt)
        self.max_age = max_age

    def sign(self, payload: bytes, now: int) -> str:
        ts: list[int] = []
        while now > 0:
            ts.append(now % 256)
            now //= 256
        timestamp = bytes(ts[::-1])
        unsigned = encode_url(payload) + b"." + encode_url(timestamp)
        return (unsigned + b"." + encode_url(hmac_sha1(self.key, unsigned))).decode("ascii")

    def open(self, token: str, now: int) -> bytes:
        parts = token.split(".")
        if len(parts) != 3:
            raise ValueError("invalid signed token")
        unsigned = (parts[0] + "." + parts[1]).encode("ascii")
        if not hmac.compare_digest(encode_url(hmac_sha1(self.key, unsigned)), parts[2].encode("ascii")):
            raise ValueError("invalid signature")
        issued = 0
        for byte in decode_url(parts[1].encode("ascii")):
            issued = issued * 256 + byte
        if now < issued or now - issued > self.max_age:
            raise ValueError("expired signature")
        return decode_url(parts[0].encode("ascii"))
