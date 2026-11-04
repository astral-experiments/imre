import ssl

context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
context.set_ciphers("ECDHE-RSA-AES128-GCM-SHA256:ECDHE-RSA-AES256-SHA384")
ciphers = context.get_ciphers()
selected = [entry for entry in ciphers if entry["protocol"] != "TLSv1.3"]
assert [entry["name"] for entry in selected] == [
    "ECDHE-RSA-AES128-GCM-SHA256", "ECDHE-RSA-AES256-SHA384",
]
for entry in selected:
    print(list(entry), entry["id"], entry["name"], entry["protocol"],
          entry["strength_bits"], entry["alg_bits"], entry["aead"],
          entry["symmetric"], entry["digest"], entry["kea"], entry["auth"])
    assert entry["description"].startswith(entry["name"])
    assert not entry["description"].endswith("\n")
count = len(ciphers)
selected[0].clear()
ciphers.clear()
fresh = context.get_ciphers()
assert len(fresh) == count and all("id" in entry for entry in fresh)


class GuardedContext(ssl.SSLContext):
    def __getattribute__(self, name):
        if name == "_ctx":
            raise AssertionError("cipher query read a subclass attribute")
        return super().__getattribute__(name)


guarded = GuardedContext(ssl.PROTOCOL_TLS_CLIENT)
assert guarded.get_ciphers() == ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT).get_ciphers()


class CurveName:
    def __init__(self, name):
        self.name = name

    def __fspath__(self):
        return self.name

    def __repr__(self):
        return "curve argument"


assert guarded.set_ecdh_curve(CurveName(b"prime256v1")) is None
try:
    guarded.set_ecdh_curve(CurveName("unknown-ecdh-test-curve"))
except ValueError as exc:
    assert str(exc) == "unknown elliptic curve name curve argument"
else:
    raise AssertionError("accepted an unsupported short name")
try:
    guarded.set_ecdh_curve(b"prime256v1\0suffix")
except ValueError as exc:
    assert str(exc) == "embedded null byte"
else:
    raise AssertionError("accepted an embedded NUL")
try:
    guarded.set_ecdh_curve("RSA")
except ssl.SSLError as exc:
    assert isinstance(exc.errno, int)
else:
    raise AssertionError("accepted a known object that is not a TLS group")
