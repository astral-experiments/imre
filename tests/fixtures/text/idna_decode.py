def decode(host: bytes, codec: str) -> str:
    return host.decode(encoding=codec, errors="strict")


for name in (b"EXAMPLE.org", b"XN--BCHER-KVA.example.", b"xn--pythn-mua.org"):
    print(decode(name, "idna"))
for invalid in (b"example.\xff", b"xn--pythn-m&ua"):
    try:
        print(decode(invalid, "idna"))
    except UnicodeDecodeError as error:
        print(error.args)
